//! VSD machine (Camelot "VSD"): two heads LH1/LH2, VSD diaphragm pumps on CAN octo-boards
//! (node 610..633, drive 1..8), Mettler scales over TCP (MT-SICS), lifts/units/line/transport via the
//! Omron PLC mailbox. Pump commands follow Camelot CVSDDispensePosition: params p1 target steps,
//! p2 whole rotations, p3 partial steps, p4..p6 speed/accel/decel in steps, p7 move-zero 0, p8 drop-back.
use crate::{model::Result,plc::PlcConfig,transport::{Bridge,Device,StopHandle},vsdcal::{cosine_positions,CalSettings,VsdCalibration}};
use serde::{Deserialize,Serialize};
use serde_json::json;
use std::{collections::HashMap,io::{BufRead,BufReader,Write},net::{TcpStream,ToSocketAddrs},path::Path,sync::{Arc,Mutex},time::{Duration,Instant}};

pub const CMD_HOME:u32=22;pub const CMD_SETPOS:u32=66;pub const CMD_INIT:u32=70;pub const CMD_STATUS:u32=71;
/// Drive parameters travel as signed 32-bit little endian (`vsd_bridge.py`): every target must fit.
pub const P_MAX:i64=i32::MAX as i64;pub const P_MIN:i64=i32::MIN as i64;
pub const CMD_DISPENSE:u32=73;pub const CMD_MOVETO:u32=74;pub const CMD_ZERO:u32=76;
const RSP_DONE:u32=2;const RSP_HOME:u32=77;
/// Update reply (1): the board answers STATUS with it (Camelot VSDDispensePosition.cpp treats it like ReadStatus).
const RSP_UPDATE:u32=1;const RSP_STATUS_UPDATE:u32=72;
/// Accepted answers to STATUS: ReadStatus (71), update (1), UpdateStatus (72); p1 is the position in each.
const STATUS_OK:[u32;3]=[CMD_STATUS,RSP_UPDATE,RSP_STATUS_UPDATE];

#[derive(Clone,Copy,Debug,Serialize,Deserialize,PartialEq,Eq,Hash)]
pub enum Head{LH1,LH2}
/// Lift height in cm: above 0, at most 100 cm (P1 = height × 10, Camelot `Lift.cpp`).
pub fn check_height(cm:f64)->Result<f64>{if !cm.is_finite() || cm<=0. || cm>100.{return Err("hoogte moet tussen 0 en 100 cm liggen".into());}Ok(cm)}
/// PLC parameter P1 for a lift height in cm (height × 10, rounded).
pub fn height_p1(cm:f64)->u32{(cm*10.).round().max(0.) as u32}
impl Head{
    pub const ALL:[Head;2]=[Head::LH1,Head::LH2];
    pub fn idx(self)->usize{match self{Head::LH1=>0,Head::LH2=>1}}
    pub fn name(self)->&'static str{match self{Head::LH1=>"LH1",Head::LH2=>"LH2"}}
}

#[derive(Clone,Debug,Serialize,Deserialize,PartialEq)]
pub struct VsdPump{pub name:String,pub node:u16,pub drive:u8,pub head:Head,#[serde(default)] pub ingredient:String,#[serde(default="yes")] pub enabled:bool,
    /// Steps per revolution of this pump; None = machine setting (`MachineConfig::spr`, 51 200).
    #[serde(default)] pub spr:Option<i64>}
fn yes()->bool{true}

#[derive(Clone,Debug,Serialize,Deserialize,PartialEq)]
pub struct ScaleNet{pub host:String,pub port:u16}

#[derive(Clone,Debug,Serialize,Deserialize,PartialEq)]
pub struct MachineConfig{
    pub can_channel:u32,pub bitrate:u32,pub plc:PlcConfig,pub scales:[ScaleNet;2],
    pub line_process:u32,pub transport_process:u32,pub barcode_process:u32,pub unit_process:[u32;2],pub lift_process:[u32;2],
    pub spr:i64,pub speed_rps:f64,pub accel_rps2:f64,pub decel_rps2:f64,pub timeout_s:f64,
    pub pumps:Vec<VsdPump>,
    /// Lift height per head in cm (Camelot: container height; PLC P1 = height × 10). None = not set:
    /// a pot on hardware then needs the pot height on the recipe.
    #[serde(default)] pub lift_height_cm:[Option<f64>;2],
    /// Barcode scanner (generic protocol), line spots and the optional ZPL label printer (host, port 9100).
    #[serde(default)] pub scanner:crate::scanner::ScannerConfig,#[serde(default)] pub line:crate::production::LineCfg,
    #[serde(default)] pub printer_host:String,#[serde(default="default_printer_port")] pub printer_port:u16,
}
fn default_printer_port()->u16{9100}
impl Default for MachineConfig{
    // Wiring as found in Camelot (TST_Guus_V4, November 2026); stored and edited in Pump Studio itself.
    fn default()->Self{Self{can_channel:0,bitrate:250000,plc:PlcConfig::default(),
        scales:[ScaleNet{host:"192.168.0.56".into(),port:23},ScaleNet{host:"192.168.0.55".into(),port:23}],
        line_process:1310,transport_process:1220,barcode_process:1190,unit_process:[1300,2300],lift_process:[1350,2350],
        spr:51200,speed_rps:1.,accel_rps2:1.,decel_rps2:1.,timeout_s:120.,pumps:default_pumps(),lift_height_cm:[None,None],scanner:Default::default(),line:Default::default(),printer_host:String::new(),printer_port:9100}}
}
/// A01..B48 on LH2 (boards 610..621), C01..D48 on LH1 (boards 622..633); 8 drives per board.
pub fn default_pumps()->Vec<VsdPump>{
    let mut v=vec![];
    for (letter,first,head) in [('A',610u16,Head::LH2),('B',616,Head::LH2),('C',622,Head::LH1),('D',628,Head::LH1)]{
        for i in 1..=48u16{v.push(VsdPump{name:format!("P_{letter}{i:02}"),node:first+(i-1)/8,drive:((i-1)%8+1) as u8,head,ingredient:format!("I_{letter}{i:02}"),enabled:true,spr:None});}
    }
    v
}
impl MachineConfig{
    pub fn load(path:&Path)->Self{std::fs::read(path).ok().and_then(|b|serde_json::from_slice(&b).ok()).unwrap_or_default()}
    pub fn save(&self,path:&Path)->Result<()>{
        self.validate()?;
        if let Some(d)=path.parent(){std::fs::create_dir_all(d).map_err(|e|e.to_string())?;}
        let tmp=path.with_extension("json.tmp");
        std::fs::write(&tmp,serde_json::to_vec_pretty(self).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
        std::fs::rename(tmp,path).map_err(|e|e.to_string())
    }
    pub fn validate(&self)->Result<()>{
        self.plc.validate()?;
        if self.spr<=0 || [self.speed_rps,self.accel_rps2,self.decel_rps2].iter().any(|v|!v.is_finite() || *v<=0. || *v>20.){return Err("Stappen/omw, snelheid en versnelling moeten positief zijn".into());}
        if !(1. ..=3600.).contains(&self.timeout_s){return Err("Time-out 1 tot 3600 s".into());}
        for (k,h) in self.lift_height_cm.iter().enumerate(){if let Some(v)=h{check_height(*v).map_err(|e|format!("Lifthoogte LH{}: {e}",k+1))?;}}
        let mut seen=HashMap::new();
        for p in &self.pumps{
            if !(1..=8).contains(&p.drive) || p.node==0 || p.node>2047{return Err(format!("{}: ongeldig CAN-adres",p.name));}
            if let Some(o)=seen.insert((p.node,p.drive),&p.name){return Err(format!("{} en {} hebben hetzelfde CAN-adres",o,p.name));}
            if p.spr.is_some_and(|v|v<=0 || v>P_MAX){return Err(format!("{}: stappen per omwenteling ongeldig",p.name));}
        }
        Ok(())
    }
    pub fn pump_index(&self,name:&str)->Option<usize>{self.pumps.iter().position(|p|p.name.eq_ignore_ascii_case(name.trim()))}
    /// Steps per revolution of pump `i` (own value, else the machine value).
    pub fn spr_of(&self,i:usize)->i64{self.pumps.get(i).and_then(|p|p.spr).filter(|v|*v>0).unwrap_or(self.spr)}
}

// ---------- CAN ----------
#[derive(Clone,Debug,PartialEq)]
pub struct Reply{pub node:u16,pub drive:u8,pub code:u32,pub error:bool,pub position:i64}
pub struct WaitOut{pub replies:Vec<Reply>,pub missing:Vec<(u16,u8)>,pub fault:Option<String>}
/// A dose block whose DISPENSE commands are out: (pump, steps), positions before, drives, answers so far, time-out.
#[derive(Debug)]
pub struct PendingBlock{pub items:Vec<(usize,i64)>,pub before:Vec<i64>,targets:Vec<(u16,u8,Vec<u32>)>,done:HashMap<(u16,u8),i64>,deadline:Instant}
pub trait CanLink:Send{
    fn send(&mut self,node:u16,drive:u8,cmd:u32,params:[i64;8])->Result<()>;
    fn wait(&mut self,targets:&[(u16,u8,Vec<u32>)],timeout_s:f64)->Result<WaitOut>;
}
/// Real boards through bridge/vsd_bridge.py (python-can + IXXAT).
pub struct BridgeCan{dev:Bridge}
impl BridgeCan{
    pub fn open(python:&Path,script:&Path,output:&Path,stop:StopHandle,cfg:&MachineConfig)->Result<Self>{
        let mut dev=Bridge::spawn(python,script,output,output,stop)?;
        dev.call("connect",json!({"channel":cfg.can_channel,"bitrate":cfg.bitrate,"timeout_s":cfg.timeout_s}))?;
        Ok(Self{dev})
    }
}
impl CanLink for BridgeCan{
    fn send(&mut self,node:u16,drive:u8,cmd:u32,params:[i64;8])->Result<()>{self.dev.call("send",json!({"node":node,"drive":drive,"cmd":cmd,"params":params})).map(|_|())}
    fn wait(&mut self,targets:&[(u16,u8,Vec<u32>)],timeout_s:f64)->Result<WaitOut>{
        let t:Vec<_>=targets.iter().map(|(n,d,c)|json!({"node":n,"drive":d,"codes":c})).collect();
        let r=self.dev.call("wait",json!({"targets":t,"timeout":timeout_s}))?;
        // The bridge ends a wait on STOP (stop generation) and has sent INIT: no DISPENSE reply will come.
        if r["stopped"]==true{return Err("Gestopt (STOP): pompen stilgezet".into());}
        let replies=r["results"].as_array().into_iter().flatten().map(|x|Reply{node:x["node"].as_u64().unwrap_or(0) as u16,drive:x["drive"].as_u64().unwrap_or(0) as u8,
            code:x["code"].as_u64().unwrap_or(0) as u32,error:x["error"].as_bool().unwrap_or(true),position:x["position"].as_i64().unwrap_or(0)}).collect();
        let missing=r["missing"].as_array().into_iter().flatten().map(|x|(x["node"].as_u64().unwrap_or(0) as u16,x["drive"].as_u64().unwrap_or(0) as u8)).collect();
        Ok(WaitOut{replies,missing,fault:r["fault"].as_str().map(String::from)})
    }
}

/// Shared simulated machine: pump shaft positions and the weight on each head's scale.
/// Delivery model as a diaphragm pump: the first half revolution of every stroke delivers, the return refills.
/// A dispense/move travels at the configured speed (capped at `max_travel_s`); weight and position follow
/// the travel, so a STOP ends a simulated dose part-way just like on the machine.
pub struct SimWorld{pub pos:HashMap<(u16,u8),i64>,pub head:HashMap<(u16,u8),Head>,pub g_per_stroke:f64,pub weight:[f64;2],pub spr:i64,pub speed_sps:f64,
    /// Drives in motion: (from, to, start, duration s).
    pub moves:HashMap<(u16,u8),(i64,i64,Instant,f64)>,
    /// Longest simulated travel per command (0.2 s keeps unit tests fast; the app uses real travel time).
    pub max_travel_s:f64}
impl SimWorld{
    pub fn new(cfg:&MachineConfig)->Arc<Mutex<Self>>{
        Arc::new(Mutex::new(Self{pos:HashMap::new(),head:HashMap::new(),g_per_stroke:0.5,weight:[100.,100.],spr:cfg.spr,speed_sps:cfg.speed_rps*cfg.spr as f64,moves:HashMap::new(),max_travel_s:0.2}.with_pumps(&cfg.pumps)))
    }
    fn with_pumps(mut self,pumps:&[VsdPump])->Self{self.set_pumps(pumps);self}
    /// Pump list changed (head edited, pump added): the scale a pump doses on follows the list.
    pub fn set_pumps(&mut self,pumps:&[VsdPump]){self.head=pumps.iter().map(|p|((p.node,p.drive),p.head)).collect();}
    fn delivered(&self,steps:i64)->f64{let c=steps.div_euclid(self.spr) as f64;let ph=steps.rem_euclid(self.spr) as f64/self.spr as f64;self.g_per_stroke*(c+(ph*2.).min(1.))}
    fn travelled(m:&(i64,i64,Instant,f64),now:Instant)->i64{
        let f=if m.3<=0.{1.}else{(now.saturating_duration_since(m.2).as_secs_f64()/m.3).min(1.)};
        m.0+((m.1-m.0) as f64*f).round() as i64
    }
    /// End a motion at `now` (complete when its time is up, part-way when stopped early).
    fn finish(&mut self,key:(u16,u8),now:Instant)->Option<i64>{
        let m=self.moves.remove(&key)?;let p=Self::travelled(&m,now);
        if p>m.0{if let Some(h)=self.head.get(&key){let d=self.delivered(p)-self.delivered(m.0);self.weight[h.idx()]+=d;}}
        self.pos.insert(key,p);Some(p)
    }
    /// Scale reading including liquid of doses still running.
    pub fn live_weight(&self,head:Head)->f64{
        let now=Instant::now();
        self.weight[head.idx()]+self.moves.iter().filter(|(k,_)|self.head.get(k)==Some(&head)).map(|(_,m)|{let p=Self::travelled(m,now);if p>m.0{self.delivered(p)-self.delivered(m.0)}else{0.}}).sum::<f64>()
    }
    pub fn moving(&self)->bool{!self.moves.is_empty()}
}
/// Replies per drive since its last command, in order (like the bridge): a board may send an error-flagged register
/// set right after "done"; the first accepted-or-error reply counts. STATUS is answered with an update (1), as the board does.
pub struct SimCan{world:Arc<Mutex<SimWorld>>,last:HashMap<(u16,u8),Vec<Reply>>,stop:Option<StopHandle>,
    /// Test hook: every dispense "done" is followed by an error-flagged zero set (seen on the machine).
    pub trailing_error:bool}
impl SimCan{
    pub fn new(world:Arc<Mutex<SimWorld>>)->Self{Self{world,last:HashMap::new(),stop:None,trailing_error:false}}
    /// STOP ends a running simulated motion within one 20 ms slice.
    pub fn with_stop(world:Arc<Mutex<SimWorld>>,stop:StopHandle)->Self{Self{world,last:HashMap::new(),stop:Some(stop),trailing_error:false}}
}
impl CanLink for SimCan{
    fn send(&mut self,node:u16,drive:u8,cmd:u32,p:[i64;8])->Result<()>{
        let mut w=self.world.lock().unwrap();let key=(node,drive);
        if !w.head.contains_key(&key){return Ok(());} // nobody answers
        let now=Instant::now();w.finish(key,now);
        let old=*w.pos.get(&key).unwrap_or(&0);
        let (code,new)=match cmd{
            CMD_DISPENSE|CMD_MOVETO=>(cmd,p[0]),CMD_SETPOS=>(cmd,p[0]),CMD_ZERO=>(cmd,0),CMD_HOME=>(RSP_HOME,0),CMD_STATUS=>(RSP_UPDATE,old),CMD_INIT=>(cmd,old),_=>(RSP_DONE,old)};
        if cmd==CMD_DISPENSE || cmd==CMD_MOVETO{
            let secs=((new-old).abs() as f64/w.speed_sps).min(w.max_travel_s);
            w.moves.insert(key,(old,new,now,secs));
        }else{w.pos.insert(key,new);}
        let mut v=vec![Reply{node,drive,code,error:false,position:new}];
        if self.trailing_error && cmd==CMD_DISPENSE{v.push(Reply{node,drive,code:0xFFFF,error:true,position:0});}
        self.last.insert(key,v);Ok(())
    }
    fn wait(&mut self,targets:&[(u16,u8,Vec<u32>)],t:f64)->Result<WaitOut>{
        let keys:Vec<(u16,u8)>=targets.iter().map(|(n,d,_)|(*n,*d)).collect();
        let deadline=Instant::now()+Duration::from_secs_f64(t.max(0.));
        // Real travel time so parallel runs (also of other, still outstanding blocks) overlap; stop-aware like the
        // CAN bridge. Drives still travelling at the time-out stay outstanding (reported missing, answer kept).
        loop{
            let now=Instant::now();
            let end={let w=self.world.lock().unwrap();keys.iter().filter_map(|k|w.moves.get(k).map(|m|m.2+Duration::from_secs_f64(m.3))).filter(|e|*e>now).min()};
            let Some(e)=end else{break};
            if self.stop.as_ref().is_some_and(|s|s.check().is_err()){
                let mut w=self.world.lock().unwrap();
                for k in &keys{if w.finish(*k,now).is_some(){self.last.remove(k);}}
                return Err("Gestopt: pomp(en) onderweg stilgezet (simulatie)".into());
            }
            if now>=deadline{break;}
            std::thread::sleep((e-now).min(deadline-now).min(Duration::from_millis(20)));
        }
        let now=Instant::now();let mut replies=vec![];let mut missing=vec![];
        let mut w=self.world.lock().unwrap();
        for (n,d,codes) in targets{
            let k=(*n,*d);
            let running=w.moves.get(&k).is_some_and(|m|m.2+Duration::from_secs_f64(m.3)>now);
            if running{missing.push(k);continue;}
            w.finish(k,now);
            let first=self.last.remove(&k).and_then(|v|v.into_iter().find(|r|r.error || (if codes.is_empty(){r.code!=RSP_UPDATE}else{codes.contains(&r.code)})));
            match first{Some(r)=>replies.push(r),None=>missing.push(k)}
        }
        Ok(WaitOut{replies,missing,fault:None})
    }
}

// ---------- Scales (Mettler MT-SICS over TCP) ----------
pub trait ScaleLink:Send{
    fn read(&mut self,stable:bool,timeout_s:f64)->Result<(f64,bool)>;
    /// As `read`, ending early (error "Gestopt") when `stop` returns true (STOP during a stable weighing).
    fn read_until(&mut self,stable:bool,timeout_s:f64,_stop:&dyn Fn()->bool)->Result<(f64,bool)>{self.read(stable,timeout_s)}
}
/// "S S      12.3456 g" -> (12.3456, true); "S D ..." dynamic; busy -> None; errors and overload -> Err.
pub fn parse_mtsics(line:&str)->Result<Option<(f64,bool)>>{
    let t:Vec<&str>=line.split_whitespace().collect();
    match t.as_slice(){
        ["S"|"SI",flag@("S"|"D"),value,unit,..]=>{
            let v:f64=value.parse().map_err(|_|format!("Onleesbare weging: {line}"))?;
            let f=match *unit{"g"=>1.,"kg"=>1000.,"mg"=>0.001,u=>return Err(format!("Onbekende eenheid {u}"))};
            Ok(Some((v*f,*flag=="S")))
        }
        ["S"|"SI","I",..]=>Ok(None),
        ["S"|"SI","+",..]=>Err("Weegschaal overbelast".into()),
        ["S"|"SI","-",..]=>Err("Weegschaal onderbelast (schaal leeg of verkeerd getarreerd)".into()),
        [e,..] if e.starts_with('E')=>Err(format!("Weegschaalfout {e}")),
        _=>Ok(None),
    }
}
pub struct MtSicsTcp{stream:TcpStream,reader:BufReader<TcpStream>}
impl MtSicsTcp{
    pub fn connect(net:&ScaleNet)->Result<Self>{
        let addr=(net.host.trim(),net.port).to_socket_addrs().map_err(|e|e.to_string())?.next().ok_or("Ongeldig weegschaaladres")?;
        let stream=TcpStream::connect_timeout(&addr,Duration::from_secs(3)).map_err(|e|format!("Weegschaal {}:{} niet bereikbaar: {e}",net.host.trim(),net.port))?;
        stream.set_read_timeout(Some(Duration::from_millis(300))).map_err(|e|e.to_string())?;
        let reader=BufReader::new(stream.try_clone().map_err(|e|e.to_string())?);
        Ok(Self{stream,reader})
    }
}
impl ScaleLink for MtSicsTcp{
    fn read(&mut self,stable:bool,timeout_s:f64)->Result<(f64,bool)>{self.read_until(stable,timeout_s,&||false)}
    fn read_until(&mut self,stable:bool,timeout_s:f64,stop:&dyn Fn()->bool)->Result<(f64,bool)>{
        // Drop late answers of an earlier (timed-out) request so they are never taken for this one.
        let _=self.stream.set_nonblocking(true);
        let mut junk=String::new();
        while matches!(self.reader.read_line(&mut junk),Ok(n) if n>0){junk.clear();}
        let _=self.stream.set_nonblocking(false);
        let _=self.stream.set_read_timeout(Some(Duration::from_millis(100)));
        let deadline=Instant::now()+Duration::from_secs_f64(timeout_s);
        // "S" waits on the balance for a stable value; "SI" answers at once.
        let cmd:&[u8]=if stable{b"S\r\n"}else{b"SI\r\n"};
        self.stream.write_all(cmd).map_err(|e|format!("Weegschaal schrijven: {e}"))?;
        let mut line=String::new();
        while Instant::now()<deadline{
            if stop(){return Err("Gestopt".into());}
            line.clear();
            match self.reader.read_line(&mut line){
                Ok(0)=>return Err("Weegschaal verbinding gesloten".into()),
                Ok(_)=>if let Some(r)=parse_mtsics(&line)?{if !stable || r.1{return Ok(r);}},
                Err(e) if matches!(e.kind(),std::io::ErrorKind::WouldBlock|std::io::ErrorKind::TimedOut)=>{}
                Err(e)=>return Err(format!("Weegschaal lezen: {e}")),
            }
            if stable && line.trim_start().starts_with("S I"){self.stream.write_all(cmd).map_err(|e|e.to_string())?;}
        }
        Err("Geen stabiele weging binnen time-out".into())
    }
}
pub struct SimScale{world:Arc<Mutex<SimWorld>>,head:Head}
impl SimScale{pub fn new(world:Arc<Mutex<SimWorld>>,head:Head)->Self{Self{world,head}}}
/// Stable unless a simulated dose on this head is still running.
impl ScaleLink for SimScale{fn read(&mut self,_s:bool,_t:f64)->Result<(f64,bool)>{let w=self.world.lock().unwrap();let moving=w.moves.keys().any(|k|w.head.get(k)==Some(&self.head));Ok((w.live_weight(self.head),!moving))}}

// ---------- Machine ----------
#[derive(Clone,Debug,Default,Serialize,Deserialize)]
pub struct MachineState{
    pub connected:bool,pub simulation:bool,pub status:String,
    pub weight:[Option<f64>;2],pub stable:[bool;2],pub tare:[f64;2],pub scale_ok:[bool;2],
    /// Last known shaft position per pump in revolutions (index as config pumps).
    pub positions:Vec<Option<f64>>,pub log:Vec<String>,
    /// Calibration of the pump shown in the Machine tab (last run or loaded file) and progress text.
    #[serde(default)] pub calibration:Option<VsdCalibration>,#[serde(default)] pub progress:String,
    #[serde(default)] pub last_dose:Option<DoseResult>,
}
pub struct Machine{pub cfg:MachineConfig,can:Box<dyn CanLink>,scales:[Option<Box<dyn ScaleLink>>;2],pub steps:HashMap<usize,i64>,pub tare:[f64;2],pub log:Vec<String>,pub world:Option<Arc<Mutex<SimWorld>>>,
    /// Last move per pump: position before, target sent as p1, position reached and (dispense) phase after SETPOS.
    pub last:HashMap<usize,MoveReport>,
    /// Pumps in a dose block that is still running (`dispense_start` … `dispense_finish`/error/abort).
    pub running:std::collections::HashSet<usize>}
/// What one Dispense/MoveTo did, in steps (exact as sent to the drive).
#[derive(Clone,Debug,Default,PartialEq,Serialize,Deserialize)]
pub struct MoveReport{pub op:String,pub before:Option<i64>,pub target:i64,pub reached:i64,pub phase:Option<i64>,pub spr:i64}
#[derive(Clone,Debug,Default,Serialize,Deserialize,PartialEq)]
pub struct DoseResult{pub pump:String,pub head:String,pub requested_g:f64,pub revolutions:f64,pub measured_g:Option<f64>,pub error_pct:Option<f64>}
#[derive(Clone,Copy,Debug,PartialEq)]
pub enum PumpOp{Dispense,MoveTo,Home,Zero,Init,Status}
/// Range check of a step value before anything is sent: Dispense forward only, MoveTo within the drive range.
pub fn check_steps(op:PumpOp,steps:i64)->Result<()>{
    match op{
        PumpOp::Dispense if steps<=0=>Err("Doseren alleen vooruit: aantal stappen moet positief zijn".into()),
        PumpOp::Dispense if steps>P_MAX=>Err(format!("Doseren: maximaal {P_MAX} stappen per opdracht")),
        PumpOp::MoveTo if !(P_MIN..=P_MAX).contains(&steps)=>Err(format!("Positie buiten drivebereik ({P_MIN} … {P_MAX} stappen)")),
        _=>Ok(()),
    }
}

impl Machine{
    /// `skip`: heads whose scale is already used by the pump bench (one TCP client per scale).
    pub fn open(cfg:MachineConfig,simulation:bool,python:&Path,script:&Path,output:&Path,stop:StopHandle,skip:[bool;2])->Result<Self>{
        cfg.validate()?;
        let mut log=vec![];
        let (can,world,scales):(Box<dyn CanLink>,Option<Arc<Mutex<SimWorld>>>,[Option<Box<dyn ScaleLink>>;2])=if simulation{
            let w=SimWorld::new(&cfg);
            (Box::new(SimCan::new(w.clone())),Some(w.clone()),[Some(Box::new(SimScale::new(w.clone(),Head::LH1))),Some(Box::new(SimScale::new(w,Head::LH2)))])
        }else{
            let can=Box::new(BridgeCan::open(python,script,output,stop,&cfg)?);
            let mut sc:[Option<Box<dyn ScaleLink>>;2]=[None,None];
            for h in Head::ALL{
                if skip[h.idx()]{let t=format!("Weegschaal {} niet verbonden: al in gebruik door de pompbank (verbreek pompbank of zet daar weegschaal uit)",h.name());crate::applog::warn("WEEGSCHAAL",&t);log.push(t);continue;}
                match MtSicsTcp::connect(&cfg.scales[h.idx()]){
                Ok(s)=>sc[h.idx()]=Some(Box::new(s)),
                Err(e)=>{crate::applog::warn("WEEGSCHAAL",&format!("{} niet verbonden: {e}",h.name()));log.push(format!("Weegschaal {} niet verbonden: {e}",h.name()))}
                
            }}
            (can,None,sc)
        };
        let mut m=Self::from_parts(cfg,can,scales,world);m.log=log;Ok(m)
    }
    /// Pump group only (no scales): CAN link opened by the caller, e.g. the CAN worker of the
    /// connection manager, so scales and PLC are opened and closed separately.
    pub fn from_parts(cfg:MachineConfig,can:Box<dyn CanLink>,scales:[Option<Box<dyn ScaleLink>>;2],world:Option<Arc<Mutex<SimWorld>>>)->Self{
        Self{cfg,can,scales,steps:HashMap::new(),tare:[0.;2],log:vec![],world,last:HashMap::new(),running:Default::default()}
    }
    fn note(&mut self,s:String){crate::applog::info("MACHINE",&s);self.log.push(s);if self.log.len()>300{self.log.remove(0);}}
    pub fn scale_ok(&self)->[bool;2]{[self.scales[0].is_some(),self.scales[1].is_some()]}
    /// Drive parameters for target `target` steps of pump `i`: p1 target, p2 whole revolutions, p3 rest steps.
    /// Split as Camelot (`VSDDispensePosition.cpp`: modf, then (int) casts): truncation toward zero, so the
    /// rest has the sign of the target (−12 800 → p2 0, p3 −12 800; −64 000 → p2 −1, p3 −12 800).
    fn params(&self,i:usize,target:i64)->[i64;8]{
        let spr=self.cfg.spr_of(i);let f=|v:f64|(v*spr as f64).round() as i64;
        [target,target/spr,target%spr,f(self.cfg.speed_rps),f(self.cfg.accel_rps2),f(self.cfg.decel_rps2),0,0]
    }
    /// Send one command to every listed pump first, then wait for all: pumps run concurrently.
    fn run(&mut self,cmds:&[(usize,u32,[i64;8],Vec<u32>)],timeout_s:f64)->Result<()>{
        for (i,cmd,p,_) in cmds{let pm=&self.cfg.pumps[*i];crate::applog::info("CAN",&format!("-> {} (node {} motor {}) opdracht {cmd} p1 {} snelheid {}",pm.name,pm.node,pm.drive,p[0],p[3]));self.can.send(pm.node,pm.drive,*cmd,*p)?;}
        let names=|me:&Self,n:u16,d:u8|me.cfg.pumps.iter().find(|p|p.node==n&&p.drive==d).map(|p|p.name.clone()).unwrap_or(format!("{n}/{d}"));
        let targets:Vec<_>=cmds.iter().map(|(i,_,_,c)|(self.cfg.pumps[*i].node,self.cfg.pumps[*i].drive,c.clone())).collect();
        let out=self.can.wait(&targets,timeout_s)?;
        for r in &out.replies{
            if let Some(i)=self.cfg.pumps.iter().position(|p|p.node==r.node&&p.drive==r.drive){self.steps.insert(i,r.position);}
            let n=names(self,r.node,r.drive);self.note(format!("<- {n} antwoord {}{} positie {}",r.code,if r.error{" FOUT"}else{""},r.position));
        }
        if let Some(f)=out.fault{return Err(format!("CAN-fout: {f}"));}
        let bad:Vec<String>=out.replies.iter().filter(|r|r.error).map(|r|names(self,r.node,r.drive)).collect();
        if !bad.is_empty(){return Err(format!("Foutmelding van {}",bad.join(", ")));}
        if !out.missing.is_empty(){let m:Vec<String>=out.missing.iter().map(|(n,d)|names(self,*n,*d)).collect();return Err(format!("Geen antwoord binnen {timeout_s:.0} s van {}",m.join(", ")));}
        Ok(())
    }
    /// Pump operation on one or more pumps at once. `steps` is exact drive steps: relative and forward
    /// only for Dispense, absolute (may go back) for MoveTo, ignored otherwise. Dispense ends with
    /// SetPosition to the cam phase within the stroke, as Camelot does, so the absolute counter jumps back.
    pub fn pumps(&mut self,idx:&[usize],op:PumpOp,steps:i64)->Result<String>{
        if idx.is_empty(){return Err("Geen pomp gekozen".into());}
        if let Some(i)=idx.iter().find(|i|**i>=self.cfg.pumps.len()){return Err(format!("Pomp {i} bestaat niet"));}
        if let Some(p)=idx.iter().map(|i|&self.cfg.pumps[*i]).find(|p|!p.enabled){return Err(format!("{} staat uit",p.name));}
        check_steps(op,steps)?;
        let names:Vec<String>=idx.iter().map(|i|self.cfg.pumps[*i].name.clone()).collect();
        let r=self.pumps_inner(idx,op,steps);
        // After a failed or stopped command the drive position is unknown: ask STATUS before the next dose.
        if r.is_err(){for i in idx{self.steps.remove(i);}}
        r?;Ok(format!("{:?} klaar: {}",op,names.join(", ")))
    }
    fn pumps_inner(&mut self,idx:&[usize],op:PumpOp,value:i64)->Result<()>{
        let t=self.cfg.timeout_s;
        match op{
            PumpOp::Status=>{let cmds:Vec<_>=idx.iter().map(|i|(*i,CMD_STATUS,[0;8],STATUS_OK.to_vec())).collect();self.run(&cmds,t.min(10.))?;}
            PumpOp::Init=>{let cmds:Vec<_>=idx.iter().map(|i|(*i,CMD_INIT,[0;8],vec![CMD_INIT])).collect();self.run(&cmds,t.min(10.))?;}
            PumpOp::Zero=>{let cmds:Vec<_>=idx.iter().map(|i|(*i,CMD_ZERO,[0;8],vec![CMD_ZERO,RSP_DONE])).collect();self.run(&cmds,t)?;}
            // Camelot sends HOME with home offset 0 and the drive's speed registers still set: send the current speed.
            PumpOp::Home=>{let cmds:Vec<_>=idx.iter().map(|i|(*i,CMD_HOME,self.params(*i,0),vec![RSP_HOME,RSP_DONE])).collect();self.run(&cmds,t)?;}
            PumpOp::MoveTo=>{
                let before:Vec<Option<i64>>=idx.iter().map(|i|self.steps.get(i).copied()).collect();
                let cmds:Vec<_>=idx.iter().map(|i|(*i,CMD_MOVETO,self.params(*i,value),vec![CMD_MOVETO,RSP_DONE])).collect();
                self.run(&cmds,t)?;
                for (i,b) in idx.iter().zip(before){let r=MoveReport{op:"Naar positie".into(),before:b,target:value,reached:self.steps.get(i).copied().unwrap_or(value),phase:None,spr:self.cfg.spr_of(*i)};self.last.insert(*i,r);}
            }
            PumpOp::Dispense=>{let items:Vec<(usize,i64)>=idx.iter().map(|i|(*i,value)).collect();self.dispense_each_inner(&items)?;}
        }
        Ok(())
    }
    /// Block phase 1: checks, STATUS of drives with unknown position, then every DISPENSE of the block is sent.
    /// The block then runs on its own; `dispense_poll` collects its replies, `dispense_finish` sends SETPOS.
    /// Other blocks (the other head) can be started and polled meanwhile: drives of different blocks are different.
    pub fn dispense_start(&mut self,items:&[(usize,i64)])->Result<PendingBlock>{
        if items.is_empty(){return Err("Geen pomp gekozen".into());}
        if let Some((i,_))=items.iter().find(|(i,_)|*i>=self.cfg.pumps.len()){return Err(format!("Pomp {i} bestaat niet"));}
        if let Some(p)=items.iter().map(|(i,_)|&self.cfg.pumps[*i]).find(|p|!p.enabled){return Err(format!("{} staat uit",p.name));}
        for (k,(i,_)) in items.iter().enumerate(){if items[..k].iter().any(|(j,_)|j==i){return Err(format!("{} twee keer in één blok",self.cfg.pumps[*i].name));}}
        for (_,v) in items{check_steps(PumpOp::Dispense,*v)?;}
        if let Some((i,_))=items.iter().find(|(i,_)|self.running.contains(i)){return Err(format!("{} doseert nog in een ander blok",self.cfg.pumps[*i].name));}
        let r=(||{
            let unknown:Vec<usize>=items.iter().map(|x|x.0).filter(|i|!self.steps.contains_key(i)).collect();
            if !unknown.is_empty(){let cmds:Vec<_>=unknown.iter().map(|i|(*i,CMD_STATUS,[0;8],STATUS_OK.to_vec())).collect();self.run(&cmds,10.)?;}
            let before:Vec<i64>=items.iter().map(|(i,_)|self.steps[i]).collect();
            if let Some(((i,v),b))=items.iter().zip(&before).find(|((_,v),b)|**b+*v>P_MAX){return Err(format!("{}: doel {} stappen buiten drivebereik (max {P_MAX})",self.cfg.pumps[*i].name,b+v));}
            for ((i,v),b) in items.iter().zip(&before){
                let p=self.params(*i,b+v);let pm=&self.cfg.pumps[*i];
                crate::applog::info("CAN",&format!("-> {} (node {} motor {}) opdracht {CMD_DISPENSE} p1 {} snelheid {}",pm.name,pm.node,pm.drive,p[0],p[3]));
                self.can.send(pm.node,pm.drive,CMD_DISPENSE,p)?;
            }
            let targets=items.iter().map(|(i,_)|(self.cfg.pumps[*i].node,self.cfg.pumps[*i].drive,vec![CMD_DISPENSE,RSP_DONE])).collect();
            Ok(PendingBlock{items:items.to_vec(),before,targets,done:HashMap::new(),deadline:Instant::now()+Duration::from_secs_f64(self.cfg.timeout_s)})
        })();
        match &r{Ok(_)=>{for (i,_) in items{self.running.insert(*i);}}Err(_)=>{for (i,_) in items{self.steps.remove(i);}}}
        r
    }
    /// Block phase 2: wait at most `slice_s` for the drives of this block that did not answer yet.
    /// Ok(true) = every drive answered. A drive error, a bus fault or the overall time-out fails this block only.
    pub fn dispense_poll(&mut self,b:&mut PendingBlock,slice_s:f64)->Result<bool>{
        let r=(||{
            let rest:Vec<(u16,u8,Vec<u32>)>=b.targets.iter().filter(|t|!b.done.contains_key(&(t.0,t.1))).cloned().collect();
            if rest.is_empty(){return Ok(true);}
            let out=self.can.wait(&rest,slice_s)?;
            let name=|me:&Self,n:u16,d:u8|me.cfg.pumps.iter().find(|p|p.node==n&&p.drive==d).map(|p|p.name.clone()).unwrap_or(format!("{n}/{d}"));
            for r in &out.replies{
                let n=name(self,r.node,r.drive);self.note(format!("<- {n} antwoord {}{} positie {}",r.code,if r.error{" FOUT"}else{""},r.position));
                if r.error{return Err(format!("Foutmelding van {n}"));}
                if let Some(i)=self.cfg.pumps.iter().position(|p|p.node==r.node&&p.drive==r.drive){self.steps.insert(i,r.position);}
                b.done.insert((r.node,r.drive),r.position);
            }
            if let Some(f)=out.fault{return Err(format!("CAN-fout: {f}"));}
            if b.done.len()==b.targets.len(){return Ok(true);}
            if Instant::now()>b.deadline{
                let m:Vec<String>=b.targets.iter().filter(|t|!b.done.contains_key(&(t.0,t.1))).map(|t|name(self,t.0,t.1)).collect();
                return Err(format!("Geen antwoord binnen {:.0} s van {}",self.cfg.timeout_s,m.join(", ")));
            }
            Ok(false)
        })();
        if r.is_err(){self.dispense_abort(b);}
        r
    }
    /// A block that ends without its replies (error, STOP): positions unknown → the next command asks STATUS.
    pub fn dispense_abort(&mut self,b:&PendingBlock){for (i,_) in &b.items{self.steps.remove(i);self.running.remove(i);}}
    /// Block phase 3: SETPOS of this block's drives to their cam phase (as `pumps(..Dispense..)`), move reports.
    pub fn dispense_finish(&mut self,b:PendingBlock)->Result<()>{
        let items=b.items.clone();for (i,_) in &items{self.running.remove(i);}
        let r=(||{
            let reached:Vec<i64>=items.iter().zip(&b.before).map(|((i,v),bf)|self.steps.get(i).copied().unwrap_or(bf+v)).collect();
            // Rebase each drive counter to its cam phase (wrapped to negative above half a turn).
            let phases:Vec<i64>=items.iter().zip(&reached).map(|((i,_),r)|{let spr=self.cfg.spr_of(*i);let mut ph=r.rem_euclid(spr);if ph>spr/2{ph-=spr;}ph}).collect();
            let cmds:Vec<_>=items.iter().zip(&phases).map(|((i,_),ph)|(*i,CMD_SETPOS,[*ph,0,*ph,0,0,0,0,0],vec![CMD_SETPOS,RSP_DONE])).collect();
            self.run(&cmds,10.)?;
            for (k,(i,v)) in items.iter().enumerate(){self.last.insert(*i,MoveReport{op:"Doseren".into(),before:Some(b.before[k]),target:b.before[k]+v,reached:reached[k],phase:Some(phases[k]),spr:self.cfg.spr_of(*i)});}
            Ok(())
        })();
        if r.is_err(){for (i,_) in &items{self.steps.remove(i);}}
        r
    }
    /// Dispense with its own number of steps per pump, all drives in one CAN block: every DISPENSE is sent
    /// first, then the replies of all drives are awaited, so the pumps run at the same time (parallel recipe
    /// step). Ends with SETPOS to the phase within the stroke, like `pumps(..Dispense..)`.
    pub fn dispense_each(&mut self,items:&[(usize,i64)])->Result<()>{
        if items.is_empty(){return Err("Geen pomp gekozen".into());}
        if let Some((i,_))=items.iter().find(|(i,_)|*i>=self.cfg.pumps.len()){return Err(format!("Pomp {i} bestaat niet"));}
        if let Some(p)=items.iter().map(|(i,_)|&self.cfg.pumps[*i]).find(|p|!p.enabled){return Err(format!("{} staat uit",p.name));}
        for (k,(i,_)) in items.iter().enumerate(){if items[..k].iter().any(|(j,_)|j==i){return Err(format!("{} twee keer in één blok",self.cfg.pumps[*i].name));}}
        for (_,v) in items{check_steps(PumpOp::Dispense,*v)?;}
        let r=self.dispense_each_inner(items);
        if r.is_err(){for (i,_) in items{self.steps.remove(i);}}
        r
    }
    fn dispense_each_inner(&mut self,items:&[(usize,i64)])->Result<()>{
        // Same three phases as the CAN worker uses for blocks of both heads.
        let mut b=self.dispense_start(items)?;
        while !self.dispense_poll(&mut b,1.)?{}
        self.dispense_finish(b)
    }
    /// Steps a dose of `grams` needs on pump `i` from its actual cam phase (calibration model, whole steps, ≥ 1).
    pub fn steps_for(&mut self,i:usize,grams:f64,cal:&VsdCalibration)->Result<i64>{
        let p=self.cfg.pumps.get(i).cloned().ok_or("Onbekende pomp")?;
        if !cal.pump.eq_ignore_ascii_case(&p.name){return Err(format!("Kalibratie hoort bij {}, niet {}",cal.pump,p.name));}
        if !self.steps.contains_key(&i){self.pumps(&[i],PumpOp::Status,0)?;}
        let spr=self.cfg.spr_of(i);let start=(self.steps[&i] as f64/spr as f64).rem_euclid(1.);
        let target=cal.target(start,grams)?;
        Ok((((target-start)*spr as f64).round() as i64).max(1))
    }
    /// Last known drive position per pump in steps (index as config pumps).
    pub fn positions(&self)->Vec<Option<i64>>{(0..self.cfg.pumps.len()).map(|i|self.steps.get(&i).copied()).collect()}
    /// Live display reading: answer within 0.4 s or skip; never blocks the worker.
    pub fn weigh_live(&mut self,head:Head)->Result<(f64,bool)>{
        self.scales[head.idx()].as_mut().ok_or("geen weegschaal")?.read(false,0.4)
    }
    pub fn weigh(&mut self,head:Head,stable:bool)->Result<(f64,bool)>{
        let t=self.cfg.timeout_s.min(60.);
        self.scales[head.idx()].as_mut().ok_or(format!("Geen weegschaal {} verbonden",head.name()))?.read(stable,t)
    }
    pub fn tare(&mut self,head:Head)->Result<f64>{let (w,_)=self.weigh(head,true)?;self.tare[head.idx()]=w;self.note(format!("{} tarra {w:.4} g",head.name()));Ok(w)}
    pub fn drain_log(&mut self)->Vec<String>{self.log.clone()}
    /// Wait `settle_s` (stop-aware), then a stable reading of that head's scale.
    fn settle_weigh(&mut self,head:Head,settle_s:f64,stop:&dyn Fn()->bool)->Result<f64>{
        let end=Instant::now()+Duration::from_secs_f64(if self.world.is_some(){0.}else{settle_s});
        while Instant::now()<end{if stop(){return Err("Gestopt".into());}std::thread::sleep(Duration::from_millis(50));}
        if stop(){return Err("Gestopt".into());}
        Ok(self.weigh(head,true)?.0)
    }
    /// Camelot calibration of one pump on the scale of its own head (see vsdcal).
    pub fn calibrate(&mut self,i:usize,s:&CalSettings,progress:&mut dyn FnMut(&str),stop:&dyn Fn()->bool)->Result<VsdCalibration>{
        let mut cal=VsdCalibration::default();self.calibrate_into(i,s,&mut cal,progress,stop)?;Ok(cal)
    }
    /// As `calibrate`, but the runs measured so far stay in `cal` when it fails or is stopped
    /// (stored as "afgebroken", never as active).
    pub fn calibrate_into(&mut self,i:usize,s:&CalSettings,cal:&mut VsdCalibration,progress:&mut dyn FnMut(&str),stop:&dyn Fn()->bool)->Result<()>{
        s.validate()?;
        let p=self.cfg.pumps.get(i).cloned().ok_or("Onbekende pomp")?;
        if !p.enabled{return Err(format!("{} staat uit",p.name));}
        if !self.scale_ok()[p.head.idx()]{return Err(format!("Geen weegschaal {} verbonden: {} doseert op {}",p.head.name(),p.name,p.head.name()));}
        let spr=self.cfg.spr_of(i);let head=p.head;
        *cal=VsdCalibration{pump:p.name.clone(),head:head.name().into(),node:p.node,drive:p.drive,simulation:self.world.is_some(),settings:s.clone(),spr,..Default::default()};
        cal.date_ms=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis()).unwrap_or(0);
        progress(&format!("{}: home",p.name));self.pumps(&[i],PumpOp::Home,0)?;
        let pos=cosine_positions(s.phase1_samples,s.phase1_max_deg);
        for run in 0..s.phase1_runs{
            let tare=self.settle_weigh(head,s.settle_s,stop)?;let mut row=vec![];
            for (k,pr) in pos.iter().enumerate(){
                if stop(){return Err("Gestopt".into());}
                if k>0{let t=(pr*spr as f64).round() as i64;self.run(&[(i,CMD_DISPENSE,self.params(i,t),vec![CMD_DISPENSE,RSP_DONE])],self.cfg.timeout_s)?;}
                let g=match self.settle_weigh(head,s.settle_s,stop){Ok(w)=>w-tare,Err(e)=>{cal.pd_runs.push(row);return Err(e);}};row.push(g);
                progress(&format!("{} fase 1 run {}/{}: punt {}/{} ({:.1}°) {:.4} g",p.name,run+1,s.phase1_runs,k+1,pos.len(),pr*360.,g));
            }
            self.note(format!("{} fase 1 run {}: {} punten, max {:.4} g",p.name,run+1,row.len(),row.iter().cloned().fold(0.,f64::max)));
            cal.pd_runs.push(row);
            // Finish the stroke and make the counter 0 again at the stroke start (Camelot: DISPENSE 1 rev, ZERO).
            self.run(&[(i,CMD_DISPENSE,self.params(i,spr),vec![CMD_DISPENSE,RSP_DONE])],self.cfg.timeout_s)?;
            self.pumps(&[i],PumpOp::Zero,0)?;
        }
        if s.phase2{
            for run in 0..s.phase2_runs{
                let mut row=vec![];
                for k in &s.stroke_samples{
                    let step:Result<f64>=(||{
                        if stop(){return Err("Gestopt".to_string());}
                        let tare=self.settle_weigh(head,0.,stop)?;
                        self.pumps(&[i],PumpOp::Dispense,*k as i64*spr)?;
                        Ok(self.settle_weigh(head,s.settle_s,stop)?-tare)
                    })();
                    let g=match step{Ok(g)=>g,Err(e)=>{if !row.is_empty(){cal.stroke_runs.push(row);}return Err(e);}};row.push(g);
                    progress(&format!("{} fase 2 run {}/{}: {} omw {:.4} g ({:.4} g/omw)",p.name,run+1,s.phase2_runs,k,g,g/ *k as f64));
                }
                self.note(format!("{} fase 2 run {}: {:?}",p.name,run+1,row.iter().map(|g|format!("{g:.4}")).collect::<Vec<_>>()));
                cal.stroke_runs.push(row);
            }
        }
        cal.finish();
        self.note(format!("{} gekalibreerd: {:.4} g per slag, {} reparaties fase 1",p.name,cal.g_per_stroke,cal.pd.iter().filter(|x|x.repaired).count()));
        Ok(())
    }
    /// Dose grams with a calibration from the pump's actual cam phase; check weight on its head's scale.
    pub fn dose_grams(&mut self,i:usize,grams:f64,cal:&VsdCalibration,settle_s:f64,stop:&dyn Fn()->bool)->Result<DoseResult>{
        self.dose_with(i,grams,cal,settle_s,None,stop).map(|r|r.0)
    }
    /// `dose_grams` for a test: `before` = gross weight to compare with (no tare weighing before the dose,
    /// "tarra per dosis" off), else a stable weighing first. Returns the result, the gross weight after the
    /// dose (None without scale) and the steps sent.
    pub fn dose_with(&mut self,i:usize,grams:f64,cal:&VsdCalibration,settle_s:f64,before:Option<f64>,stop:&dyn Fn()->bool)->Result<(DoseResult,Option<f64>,i64)>{
        let p=self.cfg.pumps.get(i).cloned().ok_or("Onbekende pomp")?;
        if !cal.pump.eq_ignore_ascii_case(&p.name){return Err(format!("Kalibratie hoort bij {}, niet {}",cal.pump,p.name));}
        if !self.steps.contains_key(&i){self.pumps(&[i],PumpOp::Status,0)?;}
        let weigh=self.scale_ok()[p.head.idx()];
        let tare=if !weigh{None}else if let Some(b)=before{Some(b)}else{Some(self.settle_weigh(p.head,0.,stop)?)};
        let spr=self.cfg.spr_of(i);
        let start=(self.steps[&i] as f64/spr as f64).rem_euclid(1.);
        let target=cal.target(start,grams)?;let delta=target-start;
        // The model gives revolutions; the drive gets whole steps (at least one).
        let steps=((delta*spr as f64).round() as i64).max(1);
        self.note(format!("{} {grams:.4} g: fase {start:.4} omw -> +{delta:.4} omw = {steps} st",p.name));
        self.pumps(&[i],PumpOp::Dispense,steps)?;
        let after=match tare{Some(_)=>Some(self.settle_weigh(p.head,settle_s,stop)?),None=>None};
        let measured=match(tare,after){(Some(t),Some(a))=>Some(a-t),_=>None};
        let r=DoseResult{pump:p.name.clone(),head:p.head.name().into(),requested_g:grams,revolutions:delta,measured_g:measured,
            error_pct:measured.map(|m|100.*(m-grams)/grams)};
        self.note(format!("{} dosis {grams:.4} g: gemeten {}",p.name,measured.map(|m|format!("{m:.4} g ({:+.2}%)",100.*(m-grams)/grams)).unwrap_or("niet gewogen".into())));
        Ok((r,after,steps))
    }
}

#[cfg(test)] mod tests{
    use super::*;
    fn sim()->Machine{Machine::open(MachineConfig::default(),true,Path::new(""),Path::new(""),Path::new(""),StopHandle::default(),[false;2]).unwrap()}
    #[test]fn default_wiring_matches_camelot(){
        let c=MachineConfig::default();assert_eq!(c.pumps.len(),192);c.validate().unwrap();
        let get=|n:&str|c.pumps[c.pump_index(n).unwrap()].clone();
        assert_eq!((get("P_A01").node,get("P_A01").drive,get("P_A01").head),(610,1,Head::LH2));
        assert_eq!((get("P_A09").node,get("P_B01").node,get("P_C01").node,get("P_D01").node,get("P_D48").node),(611,616,622,628,633));
        assert_eq!((get("P_B48").drive,get("P_C01").head),(8,Head::LH1));
        let mut bad=c.clone();bad.pumps[1].node=610;bad.pumps[1].drive=1;assert!(bad.validate().is_err());
    }
    #[test]fn mtsics_lines(){
        assert_eq!(parse_mtsics("S S      12.3456 g\r\n").unwrap(),Some((12.3456,true)));
        assert_eq!(parse_mtsics("SI D 0.0101 kg").unwrap(),Some((10.1,false)));
        assert_eq!(parse_mtsics("S I").unwrap(),None);
        assert!(parse_mtsics("S +").is_err());assert!(parse_mtsics("ES").is_err());
    }
    #[test]fn parallel_dispense_lands_on_right_scale(){
        let mut m=sim();let a=m.cfg.pump_index("P_A01").unwrap();let c=m.cfg.pump_index("P_C05").unwrap();
        m.pumps(&[a,c],PumpOp::Home,0).unwrap();
        let before=[m.weigh(Head::LH1,true).unwrap().0,m.weigh(Head::LH2,true).unwrap().0];
        m.pumps(&[a,c],PumpOp::Dispense,2*51200).unwrap();
        let lh1=m.weigh(Head::LH1,true).unwrap().0-before[0];let lh2=m.weigh(Head::LH2,true).unwrap().0-before[1];
        assert!((lh1-1.).abs()<1e-9 && (lh2-1.).abs()<1e-9,"{lh1} {lh2}");
        // Counter rebased to the cam phase after the dose.
        assert_eq!(m.steps[&a],0);
        m.pumps(&[c],PumpOp::Dispense,12800).unwrap();assert!((m.weigh(Head::LH1,true).unwrap().0-before[0]-1.25).abs()<1e-9);
        assert_eq!(m.steps[&c],12800);
    }
    #[test]fn calibrate_and_dose_in_simulation(){
        let mut m=sim();let c=m.cfg.pump_index("P_C10").unwrap();
        let s=CalSettings{phase1_samples:11,phase1_runs:1,phase2_runs:1,stroke_samples:vec![1,2,5],..Default::default()};
        let cal=m.calibrate(c,&s,&mut |_|{},&||false).unwrap();
        assert_eq!(cal.head,"LH1");assert!((cal.g_per_stroke-0.5).abs()<1e-6,"{}",cal.g_per_stroke);assert!((cal.stroke[2].g-2.5).abs()<1e-6);
        for g in [0.3,1.,2.2]{let r=m.dose_grams(c,g,&cal,0.,&||false).unwrap();assert!(r.error_pct.unwrap().abs()<0.5,"{g}: {:?}",r);}
        assert!(m.calibrate(c,&s,&mut |_|{},&||true).is_err());
    }
    #[test]fn missing_pump_reply_and_disabled_pump_are_errors(){
        let mut m=sim();m.cfg.pumps.push(VsdPump{name:"P_X".into(),node:700,drive:1,head:Head::LH1,ingredient:String::new(),enabled:true,spr:None});
        let x=m.cfg.pumps.len()-1;let e=m.pumps(&[x],PumpOp::Zero,0).unwrap_err();assert!(e.contains("P_X"),"{e}");
        m.cfg.pumps[0].enabled=false;assert!(m.pumps(&[0],PumpOp::Zero,0).is_err());
    }
    /// Records every frame sent (command and the eight drive parameters) and passes it on to the simulation.
    struct RecCan{inner:SimCan,log:Arc<Mutex<Vec<(u32,[i64;8])>>>}
    impl CanLink for RecCan{
        fn send(&mut self,n:u16,d:u8,cmd:u32,p:[i64;8])->Result<()>{self.log.lock().unwrap().push((cmd,p));self.inner.send(n,d,cmd,p)}
        fn wait(&mut self,t:&[(u16,u8,Vec<u32>)],s:f64)->Result<WaitOut>{self.inner.wait(t,s)}
    }
    #[test]fn k1_steps_reach_the_drive_exactly(){
        let cfg=MachineConfig::default();let w=SimWorld::new(&cfg);let log=Arc::new(Mutex::new(vec![]));
        let mut m=Machine::from_parts(cfg,Box::new(RecCan{inner:SimCan::new(w.clone()),log:log.clone()}),[None,None],Some(w));
        let i=m.cfg.pump_index("P_C01").unwrap();m.pumps(&[i],PumpOp::Home,0).unwrap();
        let last=|code:u32|log.lock().unwrap().iter().rev().find(|(c,_)|*c==code).map(|x|x.1).unwrap();
        // HOME: offset 0, current speed/accel/decel in steps (as Camelot's still-set registers), move-zero 0.
        assert_eq!(last(CMD_HOME),[0,0,0,51200,51200,51200,0,0]);
        m.cfg.speed_rps=0.5;m.pumps(&[i],PumpOp::Home,0).unwrap();assert_eq!(last(CMD_HOME)[3],25600);m.cfg.speed_rps=1.;
        for d in [1i64,51199,51200,51201,25600]{
            let start=m.steps[&i];m.pumps(&[i],PumpOp::Dispense,d).unwrap();
            let p=last(CMD_DISPENSE);let t=start+d;
            assert_eq!((p[0],p[1],p[2]),(t,t/51200,t%51200),"dosering {d} vanaf {start}");
            let r=m.last[&i].clone();let mut ph=t.rem_euclid(51200);if ph>25600{ph-=51200;}
            assert_eq!((r.before,r.target,r.reached,r.phase),(Some(start),t,t,Some(ph)));
            assert_eq!(last(CMD_SETPOS)[0],ph);assert_eq!(m.steps[&i],ph,"teller na SETPOS = fase binnen de slag");
        }
        m.pumps(&[i],PumpOp::MoveTo,25600).unwrap();assert_eq!(last(CMD_MOVETO)[0],25600);assert_eq!(m.steps[&i],25600);
        // Camelot split for negative targets: toward zero, rest with the sign of the target.
        for (t,p2,p3) in [(-11800,0,-11800),(-64000,-1,-12800),(102400+3,2,3),(-12800,0,-12800)]{
            m.pumps(&[i],PumpOp::MoveTo,t).unwrap();let p=last(CMD_MOVETO);assert_eq!((p[0],p[1],p[2]),(t,p2,p3),"naar {t}");}
        assert_eq!(m.steps[&i],-12800);
        // Refused before anything is sent.
        let n=log.lock().unwrap().len();
        assert!(m.pumps(&[i],PumpOp::Dispense,0).is_err());assert!(m.pumps(&[i],PumpOp::Dispense,-5).unwrap_err().contains("vooruit"));
        assert!(m.pumps(&[i],PumpOp::MoveTo,P_MAX+1).is_err());assert!(m.pumps(&[i],PumpOp::MoveTo,P_MIN-1).is_err());
        m.steps.insert(i,P_MAX-10);assert!(m.pumps(&[i],PumpOp::Dispense,11).unwrap_err().contains("drivebereik"));
        assert_eq!(log.lock().unwrap().len(),n,"niets verzonden bij ongeldige invoer");
        // Own steps per revolution of a pump is used for p2/p3 and the phase.
        m.cfg.pumps[i].spr=Some(25600);m.steps.insert(i,0);m.pumps(&[i],PumpOp::Dispense,30000).unwrap();
        let p=last(CMD_DISPENSE);assert_eq!((p[0],p[1],p[2]),(30000,1,4400));assert_eq!(m.steps[&i],4400);
    }
    #[test]fn dispense_each_sends_all_then_waits_once(){
        let cfg=MachineConfig::default();let w=SimWorld::new(&cfg);w.lock().unwrap().max_travel_s=0.3;let log=Arc::new(Mutex::new(vec![]));
        let mut m=Machine::from_parts(cfg,Box::new(RecCan{inner:SimCan::new(w.clone()),log:log.clone()}),[None,None],Some(w.clone()));
        let (b,c)=(m.cfg.pump_index("P_C02").unwrap(),m.cfg.pump_index("P_C03").unwrap());m.pumps(&[b,c],PumpOp::Home,0).unwrap();log.lock().unwrap().clear();
        let w0=w.lock().unwrap().weight[0];let t=Instant::now();
        m.dispense_each(&[(b,2*51200),(c,51200)]).unwrap();
        // Both DISPENSE frames before any wait, travel overlaps: 2 + 1 rev capped at 0,3 s each takes ~0,3 s, not 0,6 s.
        let cmds:Vec<u32>=log.lock().unwrap().iter().map(|x|x.0).collect();assert_eq!(&cmds[..2],&[CMD_DISPENSE,CMD_DISPENSE]);
        let together=t.elapsed();
        assert!((w.lock().unwrap().weight[0]-w0-1.5).abs()<1e-9,"2 × 0,5 + 1 × 0,5 g op LH1");
        assert_eq!((m.last[&b].target,m.last[&c].target),(102400,51200));
        // Relative to the same moves one after the other, so machine load cannot make the check flaky.
        let t=Instant::now();m.dispense_each(&[(b,51200)]).unwrap();m.dispense_each(&[(c,51200)]).unwrap();let apart=t.elapsed();
        assert!(together.as_secs_f64()<apart.as_secs_f64()*0.85,"samen {together:?}, na elkaar {apart:?}");
        assert!(m.dispense_each(&[(b,10),(b,20)]).is_err());assert!(m.dispense_each(&[(b,0)]).is_err());assert!(m.dispense_each(&[]).is_err());
    }
    /// Fake drive error: answers of `bad` come back as an error.
    struct ErrCan{inner:SimCan,bad:(u16,u8)}
    impl CanLink for ErrCan{
        fn send(&mut self,n:u16,d:u8,cmd:u32,p:[i64;8])->Result<()>{self.inner.send(n,d,cmd,p)}
        fn wait(&mut self,t:&[(u16,u8,Vec<u32>)],s:f64)->Result<WaitOut>{let mut o=self.inner.wait(t,s)?;for r in o.replies.iter_mut(){if (r.node,r.drive)==self.bad{r.error=true;}}Ok(o)}
    }
    fn fast(m:&MachineConfig,w:&Arc<Mutex<SimWorld>>){let mut x=w.lock().unwrap();x.max_travel_s=10.;x.speed_sps=10.*m.spr as f64;}
    #[test]fn m1_blocks_of_both_heads_run_at_the_same_time(){
        let cfg=MachineConfig::default();let w=SimWorld::new(&cfg);fast(&cfg,&w);
        let mut m=Machine::from_parts(cfg,Box::new(SimCan::new(w.clone())),[None,None],Some(w.clone()));
        let (a,b)=(m.cfg.pump_index("P_C01").unwrap(),m.cfg.pump_index("P_A01").unwrap());m.pumps(&[a,b],PumpOp::Home,0).unwrap();
        // LH1: 5 rev (0,5 s); LH2 sent while it runs: 1 rev (0,1 s) finishes first.
        let t=Instant::now();let mut long=m.dispense_start(&[(a,5*51200)]).unwrap();
        std::thread::sleep(Duration::from_millis(50));let mut short=m.dispense_start(&[(b,51200)]).unwrap();
        let (mut ta,mut tb)=(None,None);
        while ta.is_none() || tb.is_none(){
            if ta.is_none() && m.dispense_poll(&mut long,0.02).unwrap(){ta=Some(t.elapsed());}
            if tb.is_none() && m.dispense_poll(&mut short,0.02).unwrap(){tb=Some(t.elapsed());}
            assert!(t.elapsed()<Duration::from_secs(3));
        }
        let (ta,tb)=(ta.unwrap(),tb.unwrap());assert!(tb<ta,"LH2-blok eerst klaar: {tb:?} / {ta:?}");
        assert!(ta<Duration::from_millis(750),"geen optelling: {ta:?}");
        m.dispense_finish(long).unwrap();m.dispense_finish(short).unwrap();
        let x=w.lock().unwrap();assert!((x.weight[0]-100.-2.5).abs()<1e-9 && (x.weight[1]-100.-0.5).abs()<1e-9,"{:?}",x.weight);
        assert_eq!((m.last[&a].phase,m.last[&b].phase),(Some(0),Some(0)));
    }
    #[test]fn m1_drive_error_fails_only_its_block_and_stop_ends_both(){
        let cfg=MachineConfig::default();let w=SimWorld::new(&cfg);fast(&cfg,&w);
        let bad=cfg.pumps.iter().find(|p|p.name=="P_C02").map(|p|(p.node,p.drive)).unwrap();
        let mut m=Machine::from_parts(cfg,Box::new(ErrCan{inner:SimCan::new(w.clone()),bad}),[None,None],Some(w.clone()));
        let (a,b,c)=(m.cfg.pump_index("P_C02").unwrap(),m.cfg.pump_index("P_A01").unwrap(),m.cfg.pump_index("P_A02").unwrap());m.pumps(&[b,c],PumpOp::Home,0).unwrap();
        m.steps.insert(a,0);
        let mut x=m.dispense_start(&[(a,51200)]).unwrap();let mut y=m.dispense_start(&[(b,2*51200),(c,51200)]).unwrap();
        let ex=loop{match m.dispense_poll(&mut x,0.02){Ok(true)=>panic!("fout verwacht"),Ok(false)=>{},Err(e)=>break e}};assert!(ex.contains("Foutmelding van P_C02"),"{ex}");
        while !m.dispense_poll(&mut y,0.02).unwrap(){}m.dispense_finish(y).unwrap();
        // STOP with two blocks out: both end within one slice.
        let st=StopHandle::default();let mut m=Machine::from_parts(MachineConfig::default(),Box::new(SimCan::with_stop(w.clone(),st.clone())),[None,None],Some(w.clone()));
        m.pumps(&[a,b],PumpOp::Home,0).unwrap();
        let mut p=m.dispense_start(&[(a,20*51200)]).unwrap();let mut q=m.dispense_start(&[(b,20*51200)]).unwrap();
        std::thread::sleep(Duration::from_millis(100));st.stop();let t=Instant::now();
        assert!(m.dispense_poll(&mut p,0.02).is_err() && m.dispense_poll(&mut q,0.02).is_err());assert!(t.elapsed()<Duration::from_millis(200));
        assert!(!w.lock().unwrap().moving(),"beide pompen stilgezet");
    }
    #[test]fn status_answered_with_update_and_done_followed_by_error_set(){
        assert!(STATUS_OK.contains(&1) && STATUS_OK.contains(&71) && STATUS_OK.contains(&72));
        let cfg=MachineConfig::default();let w=SimWorld::new(&cfg);
        let mut can=SimCan::new(w.clone());can.trailing_error=true;
        let mut m=Machine::from_parts(cfg,Box::new(can),[None,None],Some(w.clone()));
        let a=m.cfg.pump_index("P_C01").unwrap();
        // Unknown position → STATUS first; the (simulated) board answers with an update (1): accepted, position known.
        m.pumps(&[a],PumpOp::Status,0).unwrap();assert_eq!(m.steps.get(&a),Some(&0));
        // Dispense: "done" then an error-flagged set: success, as Camelot.
        m.dispense_each(&[(a,51200)]).unwrap();assert_eq!(m.last[&a].reached,51200);
    }
}
