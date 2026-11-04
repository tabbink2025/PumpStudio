//! Omron NJ PLC mailbox over FINS/UDP, as Camelot uses it (CNJFinsUDP, ectOmronNJFins = 48).
//! One 44-word record per channel: every field a DWORD stored low word first, guarded by equal
//! first/last link counters. PC writes D(200·ch−200), reads D(200·ch−100). New command only when the
//! PLC echoes the previous send counter; a response is new when the PLC receive counter changes and
//! is acknowledged by writing that counter back.
use crate::model::Result;
use serde::{Deserialize,Serialize};
use std::{net::{Ipv4Addr,UdpSocket},time::{Duration,Instant}};

#[derive(Clone,Debug,Serialize,Deserialize,PartialEq)]
pub struct PlcConfig {
    pub ip:String,pub port:u16,pub host_port:u16,pub target_node:u8,pub host_node:u8,pub channel:u16,pub timeout_s:f64,
}
impl Default for PlcConfig {
    // Values of "PLC1 VSD" in Camelot database TST_Guus_V4 (October 2026).
    fn default()->Self{Self{ip:"10.6.1.15".into(),port:9600,host_port:9600,target_node:15,host_node:50,channel:1,timeout_s:60.}}
}
impl PlcConfig {
    pub fn validate(&self)->Result<()>{
        self.ip.trim().parse::<Ipv4Addr>().map_err(|_|"Ongeldig PLC-IP-adres".to_string())?;
        if !(1..=100).contains(&self.channel){return Err("Kanaal 1 tot 100".into());}
        if self.port==0 || !self.timeout_s.is_finite() || !(1. ..=600.).contains(&self.timeout_s){return Err("Poort en time-out (1–600 s) controleren".into());}
        Ok(())
    }
    pub fn read_addr(&self)->u16{200*self.channel-100}
    pub fn write_addr(&self)->u16{200*self.channel-200}
}

/// Poll slice of the mailbox while waiting for pick-up or a reply (FINS read of 44 words; Camelot polls continuously).
pub const MAILBOX_POLL:Duration=Duration::from_millis(15);
pub const CMD_START:u32=1;pub const CMD_RESET:u32=2;pub const CMD_ABORT:u32=5;pub const CMD_STOP:u32=8;pub const CMD_UP:u32=11;pub const CMD_DOWN:u32=12;
pub const RSP_UPDATE:u32=1;pub const RSP_STARTED:u32=2;pub const RSP_UP:u32=4;pub const RSP_DOWN:u32=5;pub const RSP_STOPPED:u32=9;pub const RSP_ABORTED:u32=10;
const RSP_ERROR:u32=0xFFFF;
const WORDS:u16=44;

#[derive(Clone,Copy,Debug,Default,PartialEq)]
pub struct Record{pub process:u32,pub code:u32,pub params:[u32;16],pub snd:u32,pub rcv:u32}

pub fn encode(link:u32,r:&Record)->Vec<u16>{
    let mut d=vec![link,r.process,r.code];d.extend(r.params);d.extend([r.snd,r.rcv,link]);
    d.iter().flat_map(|v|[(*v&0xFFFF) as u16,(*v>>16) as u16]).collect()
}
/// None when first and last link counter differ (record written while being read).
pub fn decode(w:&[u16])->Option<Record>{
    if w.len()<WORDS as usize{return None;}
    let dw=|i:usize|w[2*i] as u32|(w[2*i+1] as u32)<<16;
    if dw(0)!=dw(21){return None;}
    let mut params=[0;16];for (k,p) in params.iter_mut().enumerate(){*p=dw(3+k);}
    Some(Record{process:dw(1),code:dw(2),params,snd:dw(19),rcv:dw(20)})
}

pub trait Link:Send{
    fn read(&mut self,addr:u16,n:u16)->Result<Vec<u16>>;
    fn write(&mut self,addr:u16,data:&[u16])->Result<()>;
}

/// FINS over UDP, frame layout as COmronFinsUDPProtocol::FINS_read/FINS_write.
pub struct FinsUdp{sock:UdpSocket,da1:u8,sa1:u8,sid:u8}
impl FinsUdp{
    pub fn open(cfg:&PlcConfig)->Result<Self>{
        let ip:Ipv4Addr=cfg.ip.trim().parse().map_err(|_|"Ongeldig PLC-IP-adres".to_string())?;
        let sock=UdpSocket::bind(("0.0.0.0",cfg.host_port)).map_err(|e|format!("UDP-poort {} bezet (Camelot nog open?): {e}",cfg.host_port))?;
        sock.connect((ip,cfg.port)).map_err(|e|format!("PLC {ip}:{} niet bereikbaar: {e}",cfg.port))?;
        sock.set_read_timeout(Some(Duration::from_millis(1000))).map_err(|e|e.to_string())?;
        Ok(Self{sock,da1:cfg.target_node,sa1:cfg.host_node,sid:0})
    }
    fn header(&mut self,src:u8)->Vec<u8>{self.sid=self.sid.wrapping_add(1);vec![0x80,0,2,0,self.da1,0,0,self.sa1,0,self.sid,0x01,src]}
    fn transact(&mut self,frame:&[u8],expect:usize)->Result<Vec<u8>>{
        self.sock.send(frame).map_err(|e|format!("FINS verzenden: {e}"))?;
        let deadline=Instant::now()+Duration::from_millis(1500);let mut buf=[0u8;2048];
        while Instant::now()<deadline{
            let n=match self.sock.recv(&mut buf){Ok(n)=>n,Err(e) if matches!(e.kind(),std::io::ErrorKind::WouldBlock|std::io::ErrorKind::TimedOut)=>break,Err(e)=>return Err(format!("FINS ontvangen: {e}"))};
            // Stale reply of an earlier request: wrong SID, keep listening.
            if n<14 || buf[9]!=self.sid{continue;}
            if buf[12]!=0 || buf[13]!=0{return Err(format!("FINS-foutcode {:02X}{:02X}",buf[12],buf[13]));}
            if n!=expect{return Err(format!("FINS-antwoord {n} bytes, verwacht {expect}"));}
            return Ok(buf[..n].to_vec());
        }
        Err("PLC antwoordt niet (FINS time-out); netwerk, IP en nodes controleren".into())
    }
}
impl Link for FinsUdp{
    fn read(&mut self,addr:u16,n:u16)->Result<Vec<u16>>{
        let mut f=self.header(0x01);f.extend([0x82,(addr>>8) as u8,addr as u8,0,(n>>8) as u8,n as u8]);
        let r=self.transact(&f,14+2*n as usize)?;
        Ok((0..n as usize).map(|i|u16::from_be_bytes([r[14+2*i],r[15+2*i]])).collect())
    }
    fn write(&mut self,addr:u16,data:&[u16])->Result<()>{
        let n=data.len() as u16;let mut f=self.header(0x02);f.extend([0x82,(addr>>8) as u8,addr as u8,0,(n>>8) as u8,n as u8]);
        for w in data{f.extend(w.to_be_bytes());}
        self.transact(&f,14).map(|_|())
    }
}

/// Simulated PLC: DM memory plus a lift program in the spirit of Camelot's EmulationProtocol.
pub struct Emulator{dm:Vec<u16>,read_addr:u16,write_addr:u16,handled:u32,rcv:u32,pending:Option<(u32,u32,u32,u32)>,pub level:i32,pub fail_next:bool,pub moves:Vec<u32>,
    /// Scripted answers (process, reply code) for the next command to that process, e.g. (1350, 16) = "no pot".
    pub script:std::sync::Arc<std::sync::Mutex<Vec<(u32,u32)>>>,
    /// Scripted answers for one command of a process: (process, command, reply code).
    pub script_cmd:std::sync::Arc<std::sync::Mutex<Vec<(u32,u32,u32)>>>,
    /// Test hook: before the next real answer, publish an old answer (this code) carrying the previous send counter.
    pub stale_reply:Option<u32>}
impl Emulator{
    pub fn new(cfg:&PlcConfig)->Self{
        let mut e=Self{dm:vec![0;65536],read_addr:cfg.read_addr(),write_addr:cfg.write_addr(),handled:0,rcv:0,pending:None,level:0,fail_next:false,moves:vec![],script:Default::default(),script_cmd:Default::default(),stale_reply:None};
        e.publish(&Record::default());e
    }
    fn publish(&mut self,r:&Record){let w=encode(1,r);let a=self.read_addr as usize;self.dm[a..a+w.len()].copy_from_slice(&w);}
    /// Take a new command from the write area; answer after a few polls (lift travel).
    fn step(&mut self){
        let a=self.write_addr as usize;
        if let Some(r)=decode(&self.dm[a..a+WORDS as usize]){
            if r.snd!=self.handled{
                self.handled=r.snd;self.moves.push(r.code);
                let scripted={let mut s=self.script_cmd.lock().unwrap();s.iter().position(|x|x.0==r.process && x.1==r.code).map(|i|s.remove(i).2)}
                    .or_else(||{let mut s=self.script.lock().unwrap();s.iter().position(|x|x.0==r.process).map(|i|s.remove(i).1)});
                let (code,level)=if std::mem::take(&mut self.fail_next){(RSP_ERROR,self.level)}else if let Some(c)=scripted{(c,self.level)}else{match r.code{
                    CMD_START if r.process%1000!=350=>(RSP_STARTED,self.level),CMD_STOP if r.process%1000!=350=>(RSP_STOPPED,self.level),
                    CMD_START=>(RSP_UP,r.params[0] as i32),CMD_UP=>(RSP_UP,1000),CMD_STOP=>(RSP_STOPPED,0),CMD_DOWN=>(RSP_DOWN,0),CMD_ABORT=>(RSP_ABORTED,self.level),_=>(RSP_STARTED,self.level)}};
                self.pending=Some((r.process,code,level as u32,3));
                self.publish(&Record{snd:self.handled,rcv:self.rcv,..Default::default()});
            }
        }
        if let Some((p,code,level,left))=self.pending{
            if left>0{self.pending=Some((p,code,level,left-1));return;}
            if let Some(c)=self.stale_reply.take(){
                self.rcv+=1;self.publish(&Record{process:p,code:c,snd:self.handled.wrapping_sub(1),rcv:self.rcv,..Default::default()});
                self.pending=Some((p,code,level,0));return;
            }
            self.pending=None;self.rcv+=1;self.level=level as i32;
            let mut params=[0;16];params[0]=if code==RSP_ERROR{1<<3}else{level};
            self.publish(&Record{process:p,code,params,snd:self.handled,rcv:self.rcv});
        }
    }
}
impl Link for Emulator{
    fn read(&mut self,addr:u16,n:u16)->Result<Vec<u16>>{self.step();Ok(self.dm[addr as usize..(addr+n) as usize].to_vec())}
    fn write(&mut self,addr:u16,data:&[u16])->Result<()>{self.dm[addr as usize..addr as usize+data.len()].copy_from_slice(data);Ok(())}
}

pub struct Mailbox{link:Box<dyn Link>,read_addr:u16,write_addr:u16,link_count:u32,snd:Record,synced:bool,pub log:Vec<String>}
impl Mailbox{
    pub fn new(link:Box<dyn Link>,cfg:&PlcConfig)->Self{Self{link,read_addr:cfg.read_addr(),write_addr:cfg.write_addr(),link_count:0,snd:Record::default(),synced:false,log:vec![]}}
    fn note(&mut self,s:String){crate::applog::info("PLC",&s);self.log.push(s);if self.log.len()>200{self.log.remove(0);}}
    fn read_rec(&mut self)->Result<Record>{
        for _ in 0..3{if let Some(r)=decode(&self.link.read(self.read_addr,WORDS)?){return Ok(r);}}
        Err("PLC-record steeds half geschreven (linkCount ongelijk)".into())
    }
    fn write_rec(&mut self)->Result<()>{
        self.link_count=self.link_count.wrapping_add(1).max(1);
        let w=encode(self.link_count,&self.snd);self.link.write(self.write_addr,&w)
    }
    /// One poll: returns a new PLC response (already acknowledged). First poll adopts the PLC counters
    /// like Camelot after a restart, so no old command is repeated.
    /// Fire-and-forget command (Camelot sends Reset/Abort/Hold without waiting for a reply).
    pub fn post(&mut self,process:u32,cmd:u32,params:&[u32],timeout:Duration)->Result<()>{self.post_until(process,cmd,params,timeout,&||false)}
    /// `post` whose wait for the PLC to pick up the previous command ends on `stop` (STOP/Esc).
    pub fn post_until(&mut self,process:u32,cmd:u32,params:&[u32],timeout:Duration,stop:&dyn Fn()->bool)->Result<()>{
        let start=Instant::now();
        loop{
            self.poll()?;
            if self.read_rec()?.snd==self.snd.snd{break;}
            if start.elapsed()>timeout{return Err("PLC heeft vorige opdracht nog niet opgepakt".into());}
            if stop(){return Err("Gestopt".into());}
            std::thread::sleep(MAILBOX_POLL);
        }
        self.snd.process=process;self.snd.code=cmd;self.snd.params=[0;16];
        for (k,p) in params.iter().take(16).enumerate(){self.snd.params[k]=*p;}
        self.snd.snd=self.snd.snd.wrapping_add(1);self.write_rec()?;
        self.note(format!("-> proces {process} opdracht {cmd} (zonder wachten)"));Ok(())
    }
    pub fn poll(&mut self)->Result<Option<Record>>{
        let r=self.read_rec()?;
        if !self.synced{
            self.snd.snd=r.snd;self.snd.rcv=r.rcv;self.synced=true;self.write_rec()?;
            self.note(format!("Verbonden: tellers PLC verzend {} / antwoord {}",r.snd,r.rcv));return Ok(None);
        }
        if r.rcv==self.snd.rcv{return Ok(None);}
        self.snd.rcv=r.rcv;self.write_rec()?;
        if r.process==0xFFFF || r.process==0x7FFF{return Ok(None);}
        Ok(Some(r))
    }
    /// Send one command and wait for an accepted response of the same process (any non-update
    /// response when `expect` is empty). `stop` aborts the wait; caller decides about sending Abort.
    pub fn command(&mut self,process:u32,cmd:u32,param1:u32,expect:&[u32],timeout:Duration,stop:&dyn Fn()->bool,on:&mut dyn FnMut(&Record))->Result<u32>{
        self.command_with(process,cmd,&[param1],expect,timeout,stop,on)
    }
    /// Same as `command` with up to 16 parameters (e.g. transport P9/P10).
    pub fn command_with(&mut self,process:u32,cmd:u32,params:&[u32],expect:&[u32],timeout:Duration,stop:&dyn Fn()->bool,on:&mut dyn FnMut(&Record))->Result<u32>{
        let start=Instant::now();
        loop{
            self.poll()?.iter().for_each(|r|on(r));
            if self.read_rec()?.snd==self.snd.snd{break;}
            if start.elapsed()>timeout{return Err("PLC heeft vorige opdracht nog niet opgepakt".into());}
            if stop(){return Err("Gestopt".into());}
            std::thread::sleep(MAILBOX_POLL);
        }
        self.snd.process=process;self.snd.code=cmd;self.snd.params=[0;16];for (k,p) in params.iter().take(16).enumerate(){self.snd.params[k]=*p;}let param1=self.snd.params[0];
        self.snd.snd=self.snd.snd.wrapping_add(1);self.write_rec()?;
        self.note(format!("-> proces {process} opdracht {cmd} parameter {param1}"));
        loop{
            if let Some(r)=self.poll()?{
                on(&r);
                // Only a reply that belongs to the command just sent: the PLC record carries the send counter it
                // handled. An older answer of the same process (e.g. after a time-out) is never taken for this one.
                if r.process==process && r.snd!=self.snd.snd{self.note(format!("<- proces {process} antwoord {} hoort bij opdracht {} (verzonden {}): genegeerd",r.code,r.snd,self.snd.snd));continue;}
                if r.process==process{
                    if r.code==RSP_ERROR{
                        let e=format!("PLC-fout proces {process}: bits {:08X} {:08X} {:08X}, ID's {:?}",r.params[0],r.params[1],r.params[2],&r.params[6..16]);
                        self.note(e.clone());return Err(e);
                    }
                    self.note(format!("<- proces {process} antwoord {} resultaat {}",r.code,r.params[0] as i32));
                    if (expect.is_empty() && r.code!=RSP_UPDATE) || expect.contains(&r.code){return Ok(r.code);}
                }else{self.note(format!("<- proces {} antwoord {} (niet voor lift)",r.process,r.code));}
            }
            if stop(){return Err("Gestopt".into());}
            if start.elapsed()>timeout{return Err(format!("Geen antwoord van PLC binnen {} s",timeout.as_secs()));}
            std::thread::sleep(MAILBOX_POLL);
        }
    }
}

#[cfg(test)] mod tests{
    use super::*;
    fn never()->bool{false}
    #[test]fn record_roundtrip_and_guard(){
        let mut r=Record{process:1350,code:CMD_START,snd:70000,rcv:3,..Default::default()};r.params[0]=1234;
        let mut w=encode(9,&r);assert_eq!(w.len(),44);assert_eq!((w[2],w[3]),(1350,0));assert_eq!((w[38],w[39]),((70000u32&0xFFFF) as u16,1));
        assert_eq!(decode(&w),Some(r));w[42]=8;assert_eq!(decode(&w),None);
        let c=PlcConfig::default();assert_eq!((c.write_addr(),c.read_addr()),(0,100));
    }
    #[test]fn lift_up_down_through_emulator(){
        let cfg=PlcConfig::default();let mut m=Mailbox::new(Box::new(Emulator::new(&cfg)),&cfg);
        let t=Duration::from_secs(5);
        assert_eq!(m.command(1350,CMD_START,1200,&[RSP_UP,RSP_STARTED],t,&never,&mut |_|{}).unwrap(),RSP_UP);
        assert_eq!(m.command(1350,CMD_STOP,0,&[RSP_DOWN,RSP_STOPPED,RSP_STARTED],t,&never,&mut |_|{}).unwrap(),RSP_STOPPED);
        assert_eq!(m.command(2350,CMD_UP,0,&[RSP_UP,RSP_STARTED],t,&never,&mut |_|{}).unwrap(),RSP_UP);
    }
    #[test]fn reply_must_belong_to_the_sent_command(){
        // An old "16" of the same process (previous send counter) arrives first: ignored, the real answer counts.
        let cfg=PlcConfig::default();let mut e=Emulator::new(&cfg);e.stale_reply=Some(16);let mut m=Mailbox::new(Box::new(e),&cfg);
        assert_eq!(m.command_with(1350,CMD_START,&[150],&[],Duration::from_secs(5),&never,&mut |_|{}).unwrap(),RSP_UP);
        assert!(m.log.iter().any(|l|l.contains("antwoord 16") && l.contains("genegeerd")),"{:?}",m.log);
    }
    #[test]fn plc_error_record_is_reported(){
        let cfg=PlcConfig::default();let mut e=Emulator::new(&cfg);e.fail_next=true;let mut m=Mailbox::new(Box::new(e),&cfg);
        let err=m.command(1350,CMD_UP,0,&[RSP_UP],Duration::from_secs(5),&never,&mut |_|{}).unwrap_err();assert!(err.contains("PLC-fout"),"{err}");
    }
    /// PLC that stores what is written but never takes a command or answers.
    struct DeadPlc{dm:Vec<u16>}
    impl Link for DeadPlc{
        fn read(&mut self,addr:u16,n:u16)->Result<Vec<u16>>{Ok(self.dm[addr as usize..(addr+n) as usize].to_vec())}
        fn write(&mut self,_addr:u16,_d:&[u16])->Result<()>{Ok(())}
    }
    #[test]fn stop_ends_waits_on_a_silent_plc(){
        use std::sync::{Arc,atomic::{AtomicBool,Ordering}};
        let cfg=PlcConfig::default();let mut m=Mailbox::new(Box::new(DeadPlc{dm:vec![0;65536]}),&cfg);
        let flag=Arc::new(AtomicBool::new(false));let f=flag.clone();let stop=move||f.load(Ordering::SeqCst);
        let set=|ms:u64|{let fl=flag.clone();std::thread::spawn(move||{std::thread::sleep(Duration::from_millis(ms));fl.store(true,Ordering::SeqCst);});};
        // Command wait for a reply that never comes.
        set(200);let t=Instant::now();
        let e=m.command(1350,CMD_UP,0,&[RSP_UP],Duration::from_secs(30),&stop,&mut |_|{}).unwrap_err();
        assert_eq!(e,"Gestopt");assert!(t.elapsed()<Duration::from_secs(1),"{:?}",t.elapsed());
        // Fire-and-forget Reset waits until the PLC took the previous command: never, so STOP ends it.
        flag.store(false,Ordering::SeqCst);set(200);let t=Instant::now();
        let e=m.post_until(1300,CMD_RESET,&[],Duration::from_secs(30),&stop).unwrap_err();
        assert_eq!(e,"Gestopt");assert!(t.elapsed()<Duration::from_secs(1),"{:?}",t.elapsed());
    }
    #[test]fn fins_udp_frames_against_fake_plc(){
        // Fake PLC on loopback: decodes FINS DM read/write and serves an Emulator.
        let server=UdpSocket::bind("127.0.0.1:0").unwrap();let port=server.local_addr().unwrap().port();
        let mut cfg=PlcConfig{ip:"127.0.0.1".into(),port,host_port:0,..Default::default()};cfg.channel=2;
        let mut emu=Emulator::new(&cfg);
        std::thread::spawn(move||{let mut buf=[0u8;2048];loop{
            let Ok((n,from))=server.recv_from(&mut buf) else{return};let f=&buf[..n];
            assert_eq!((f[0],f[2],f[4],f[7],f[10],f[12]),(0x80,2,15,50,1,0x82));
            let addr=u16::from_be_bytes([f[13],f[14]]);let cnt=u16::from_be_bytes([f[16],f[17]]);
            let mut r=vec![0xC0,0,2,0,f[7],0,0,f[4],0,f[9],1,f[11],0,0];
            if f[11]==1{for w in emu.read(addr,cnt).unwrap(){r.extend(w.to_be_bytes());}}
            else{let d:Vec<u16>=f[18..].chunks(2).map(|c|u16::from_be_bytes([c[0],c[1]])).collect();assert_eq!(d.len(),cnt as usize);emu.write(addr,&d).unwrap();}
            server.send_to(&r,from).unwrap();}});
        let mut m=Mailbox::new(Box::new(FinsUdp::open(&cfg).unwrap()),&cfg);
        assert_eq!(m.command(2350,CMD_DOWN,0,&[RSP_DOWN],Duration::from_secs(5),&never,&mut |_|{}).unwrap(),RSP_DOWN);
    }
}
