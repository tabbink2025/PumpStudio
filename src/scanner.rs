//! Barcode scanner, "generiek" protocol as Camelot `GenericBarcodeReader`: send the trigger-on text (default `BA`),
//! the scanner answers `<STX>code<ETX><CR><LF>` or `<STX><NAK>` (no-read); then the trigger-off text (`B`).
//! Links: TCP directly, COM through `bridge/scanner_bridge.py` (pyserial), or a simulation that returns the code of
//! the pot the line model put on the scan spot (with test hooks for no-read and unknown codes).
use crate::{model::Result,transport::{Bridge,Device,StopHandle}};
use serde::{Deserialize,Serialize};
use serde_json::json;
use std::{collections::VecDeque,io::{Read,Write},net::{TcpStream,ToSocketAddrs},path::Path,sync::{Arc,Mutex},time::{Duration,Instant}};

pub const STX:u8=0x02;pub const ETX:u8=0x03;pub const NAK:u8=0x15;

#[derive(Clone,Copy,Debug,Serialize,Deserialize,PartialEq,Eq,Default)]
pub enum ScanKind{#[default]Tcp,Com}
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq)]
#[serde(default)]
pub struct ScannerConfig{
    pub kind:ScanKind,pub host:String,pub port:u16,pub com:String,pub baud:u32,
    /// Trigger texts; `\r`, `\n` and `\xNN` are understood.
    pub trigger_on:String,pub trigger_off:String,
    /// Wait for an answer per attempt and the number of attempts (Camelot: 10 s, 2).
    pub timeout_s:f64,pub retries:u32,
}
impl Default for ScannerConfig{fn default()->Self{Self{kind:ScanKind::Tcp,host:String::new(),port:9004,com:"COM1".into(),baud:9600,trigger_on:"BA".into(),trigger_off:"B".into(),timeout_s:10.,retries:2}}}
impl ScannerConfig{
    pub fn validate(&self)->Result<()>{
        match self.kind{
            ScanKind::Tcp=>{if self.host.trim().is_empty() || self.port==0{return Err("Scanner: IP-adres en poort invullen".into());}}
            ScanKind::Com=>{if self.com.trim().is_empty() || self.baud==0{return Err("Scanner: COM-poort en baudrate invullen".into());}}
        }
        if !self.timeout_s.is_finite() || !(0.5..=60.).contains(&self.timeout_s){return Err("Scanner: time-out 0,5 tot 60 s".into());}
        if !(1..=10).contains(&self.retries){return Err("Scanner: 1 tot 10 pogingen".into());}
        if unescape(&self.trigger_on).is_empty(){return Err("Scanner: trigger-tekst invullen".into());}
        Ok(())
    }
    pub fn endpoint(&self)->String{match self.kind{ScanKind::Tcp=>format!("{}:{}",self.host.trim(),self.port),ScanKind::Com=>format!("{} {} baud",self.com.trim(),self.baud)}}
}

#[derive(Clone,Debug,PartialEq)]
pub enum ScanResult{Code(String),NoRead}
impl ScanResult{pub fn text(&self)->String{match self{ScanResult::Code(c)=>c.clone(),ScanResult::NoRead=>"geen code (no-read)".into()}}}

/// `\r`, `\n`, `\t`, `\xNN` in a trigger text.
pub fn unescape(s:&str)->Vec<u8>{
    let b=s.as_bytes();let mut out=vec![];let mut i=0;
    while i<b.len(){
        if b[i]==b'\\' && i+1<b.len(){
            match b[i+1]{b'r'=>{out.push(b'\r');i+=2;continue}b'n'=>{out.push(b'\n');i+=2;continue}b't'=>{out.push(b'\t');i+=2;continue}
                b'x' if i+3<b.len()=>{if let Some(v)=std::str::from_utf8(&b[i+2..i+4]).ok().and_then(|h|u8::from_str_radix(h,16).ok()){out.push(v);i+=4;continue}}
                _=>{}}
        }
        out.push(b[i]);i+=1;
    }
    out
}
/// Visible form of raw scanner bytes (control characters as <STX> etc.).
pub fn show(raw:&[u8])->String{
    raw.iter().map(|c|match *c{STX=>"<STX>".into(),ETX=>"<ETX>".into(),NAK=>"<NAK>".into(),b'\r'=>"<CR>".into(),b'\n'=>"<LF>".into(),c if c<0x20 || c>=0x7f=>format!("<{c:02X}>"),c=>(c as char).to_string()}).collect()
}
/// A complete answer in `raw`, if any: `<STX>code<ETX>` (also without STX), `<STX><NAK>` or `<NAK>`, or a text line
/// ending in CR/LF.
pub fn parse(raw:&[u8])->Option<ScanResult>{
    let start=raw.iter().position(|c|*c==STX).map(|i|i+1).unwrap_or(0);
    let body=&raw[start..];
    if body.first()==Some(&NAK) || (start==0 && raw.contains(&NAK)){return Some(ScanResult::NoRead);}
    let end=body.iter().position(|c|*c==ETX).or_else(||body.iter().position(|c|*c==b'\r' || *c==b'\n'))?;
    let code=String::from_utf8_lossy(&body[..end]).trim().to_string();
    if code.is_empty(){return if body[end]==ETX{Some(ScanResult::NoRead)}else{None};}
    if code.eq_ignore_ascii_case("NOREAD"){return Some(ScanResult::NoRead);}
    Some(ScanResult::Code(code))
}

/// One trigger → answer on a scanner link. A time-out without a complete answer is a no-read.
pub trait ScanLink:Send{fn read(&mut self,cfg:&ScannerConfig,stop:&dyn Fn()->bool)->Result<(ScanResult,String)>;}

pub struct TcpScanner{s:TcpStream}
impl TcpScanner{
    pub fn open(host:&str,port:u16)->Result<Self>{
        let addr=(host.trim(),port).to_socket_addrs().map_err(|e|format!("Scanner {host}:{port}: {e}"))?.next().ok_or("Scanner: geen adres")?;
        let s=TcpStream::connect_timeout(&addr,Duration::from_secs(3)).map_err(|e|format!("Scanner {host}:{port}: {e}"))?;
        s.set_read_timeout(Some(Duration::from_millis(50))).map_err(|e|e.to_string())?;s.set_nodelay(true).ok();Ok(Self{s})
    }
}
impl ScanLink for TcpScanner{
    fn read(&mut self,cfg:&ScannerConfig,stop:&dyn Fn()->bool)->Result<(ScanResult,String)>{
        // Drop anything left from before, then trigger.
        let mut junk=[0u8;256];while let Ok(n)=self.s.read(&mut junk){if n==0{return Err("Scanner: verbinding gesloten".into());}}
        self.s.write_all(&unescape(&cfg.trigger_on)).map_err(|e|format!("Scanner: {e}"))?;
        let end=Instant::now()+Duration::from_secs_f64(cfg.timeout_s);let mut raw=vec![];let mut buf=[0u8;256];
        let out=loop{
            if let Some(r)=parse(&raw){break Ok(r);}
            if stop(){break Err("Gestopt".to_string());}
            if Instant::now()>end{break Ok(ScanResult::NoRead);}
            match self.s.read(&mut buf){Ok(0)=>break Err("Scanner: verbinding gesloten".into()),Ok(n)=>raw.extend_from_slice(&buf[..n]),
                Err(e) if matches!(e.kind(),std::io::ErrorKind::WouldBlock|std::io::ErrorKind::TimedOut)=>{},Err(e)=>break Err(format!("Scanner: {e}"))}
        };
        let off=unescape(&cfg.trigger_off);if !off.is_empty(){let _=self.s.write_all(&off);}
        out.map(|r|(r,if raw.is_empty(){"(geen antwoord)".into()}else{show(&raw)}))
    }
}

/// COM port through the python bridge (pyserial): the bridge does trigger, read and trigger-off.
pub struct ComScanner{b:Bridge}
impl ComScanner{
    pub fn open(python:&Path,script:&Path,output:&Path,cfg:&ScannerConfig,stop:StopHandle)->Result<Self>{
        let dir=script.parent().unwrap_or(Path::new("."));
        let mut b=Bridge::spawn(python,script,dir,output,stop)?;
        b.call("connect",json!({"port":cfg.com.trim(),"baud":cfg.baud}))?;Ok(Self{b})
    }
}
impl ScanLink for ComScanner{
    fn read(&mut self,cfg:&ScannerConfig,_stop:&dyn Fn()->bool)->Result<(ScanResult,String)>{
        let r=self.b.call("read",json!({"on":unescape(&cfg.trigger_on),"off":unescape(&cfg.trigger_off),"timeout":cfg.timeout_s}))?;
        let raw:Vec<u8>=r["bytes"].as_array().map(|a|a.iter().filter_map(|v|v.as_u64()).map(|v|v as u8).collect()).unwrap_or_default();
        Ok((parse(&raw).unwrap_or(ScanResult::NoRead),if raw.is_empty(){"(geen antwoord)".into()}else{show(&raw)}))
    }
}

/// Simulated scan spot: `feed` = the pots the operator put on the line, in order (the next read returns the first
/// and takes it). Test hooks: `no_reads` = the next n reads give `<STX><NAK>`; `inject` = codes returned before the feed.
#[derive(Default,Debug)]
pub struct SimScan{pub feed:VecDeque<String>,pub no_reads:u32,pub inject:VecDeque<String>,pub reads:u32}
pub struct SimScanner{pub line:Arc<Mutex<SimScan>>}
impl ScanLink for SimScanner{
    fn read(&mut self,_cfg:&ScannerConfig,stop:&dyn Fn()->bool)->Result<(ScanResult,String)>{
        std::thread::sleep(Duration::from_millis(20));if stop(){return Err("Gestopt".into());}
        let mut l=self.line.lock().unwrap();l.reads+=1;
        if l.no_reads>0{l.no_reads-=1;return Ok((ScanResult::NoRead,"<STX><NAK>".into()));}
        let code=l.inject.pop_front().or_else(||l.feed.pop_front());
        Ok(match code{Some(c)=>{let raw=format!("<STX>{c}<ETX><CR><LF>");(ScanResult::Code(c),raw)}None=>(ScanResult::NoRead,"(geen antwoord)".into())})
    }
}

#[cfg(test)] mod tests{
    use super::*;
    use std::net::TcpListener;
    #[test]fn parse_generic_answers(){
        assert_eq!(parse(b"\x020000000042\x03\r\n"),Some(ScanResult::Code("0000000042".into())));
        assert_eq!(parse(b"\x02\x15"),Some(ScanResult::NoRead));
        assert_eq!(parse(b"\x020000"),None,"nog niet compleet");
        assert_eq!(parse(b"ABC123\r\n"),Some(ScanResult::Code("ABC123".into())));
        assert_eq!(parse(b"\x02NOREAD\x03"),Some(ScanResult::NoRead));
        assert_eq!(unescape("BA\\r\\x1b"),vec![b'B',b'A',b'\r',0x1b]);assert_eq!(show(b"\x02A\x03\r\n"),"<STX>A<ETX><CR><LF>");
    }
    /// Fake TCP scanner: answers the trigger with a code, then a no-read, and records what it got.
    #[test]fn tcp_scanner_against_fake(){
        let l=TcpListener::bind("127.0.0.1:0").unwrap();let port=l.local_addr().unwrap().port();
        let got=Arc::new(Mutex::new(Vec::<u8>::new()));let g=got.clone();
        std::thread::spawn(move||{let (mut s,_)=l.accept().unwrap();let mut buf=[0u8;64];let mut k=0;
            loop{let n=match s.read(&mut buf){Ok(0)|Err(_)=>return,Ok(n)=>n};g.lock().unwrap().extend_from_slice(&buf[..n]);
                if buf[..n].starts_with(b"BA"){k+=1;let _=s.write_all(if k==1{b"\x02" as &[u8]}else{b"\x02\x15"});if k==1{std::thread::sleep(Duration::from_millis(30));let _=s.write_all(b"0000000007\x03\r\n");}}}});
        let cfg=ScannerConfig{host:"127.0.0.1".into(),port,timeout_s:2.,..Default::default()};
        let mut sc=TcpScanner::open("127.0.0.1",port).unwrap();
        let (r,raw)=sc.read(&cfg,&||false).unwrap();assert_eq!(r,ScanResult::Code("0000000007".into()),"{raw}");
        let (r,_)=sc.read(&cfg,&||false).unwrap();assert_eq!(r,ScanResult::NoRead);
        std::thread::sleep(Duration::from_millis(50));let g=got.lock().unwrap().clone();assert!(g.starts_with(b"BAB"),"trigger aan, dan uit: {:?}",show(&g));
    }
    #[test]fn tcp_scanner_times_out_and_stop_ends_wait(){
        let l=TcpListener::bind("127.0.0.1:0").unwrap();let port=l.local_addr().unwrap().port();
        std::thread::spawn(move||{let (s,_)=l.accept().unwrap();std::thread::sleep(Duration::from_secs(5));drop(s);});
        let cfg=ScannerConfig{host:"127.0.0.1".into(),port,timeout_s:0.5,..Default::default()};
        let mut sc=TcpScanner::open("127.0.0.1",port).unwrap();let t=Instant::now();
        assert_eq!(sc.read(&cfg,&||false).unwrap().0,ScanResult::NoRead);assert!(t.elapsed()<Duration::from_secs(1));
        let cfg=ScannerConfig{timeout_s:30.,..cfg};let t0=Instant::now();
        assert_eq!(sc.read(&cfg,&move||t0.elapsed()>Duration::from_millis(200)).unwrap_err(),"Gestopt");assert!(t0.elapsed()<Duration::from_secs(1));
    }
}
