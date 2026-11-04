//! Application log: one file per day (`logs/pumpstudio_YYYYMMDD.log`) plus an in-memory tail for the
//! Logboek tab. Every subsystem writes here: actions and results, pump bench, CAN boards, PLC, scales,
//! machine and the stderr of the Python bridges. Line format:
//! `2026-11-02 15:23:51.123  WARN   CAN        message`
use std::{io::Write,path::PathBuf,sync::{Mutex,OnceLock},time::{SystemTime,UNIX_EPOCH}};

#[derive(Clone,Copy,Debug,PartialEq,Eq,PartialOrd,Ord)]
pub enum Level{Info,Warn,Error}
impl Level{pub fn name(self)->&'static str{match self{Level::Info=>"INFO",Level::Warn=>"WARN",Level::Error=>"FOUT"}}}

#[derive(Clone,Debug)]
pub struct Line{pub time:String,pub level:Level,pub area:String,pub text:String}
impl Line{pub fn render(&self)->String{format!("{}  {:<5} {:<10} {}",self.time,self.level.name(),self.area,self.text)}}

struct Log{dir:Option<PathBuf>,tail:Vec<Line>,seq:u64}
const TAIL:usize=3000;
fn log()->&'static Mutex<Log>{static L:OnceLock<Mutex<Log>>=OnceLock::new();L.get_or_init(||Mutex::new(Log{dir:None,tail:vec![],seq:0}))}

/// Local time for the Netherlands (CET/CEST), same rule as the report.
fn stamp()->(String,String){stamp_at(SystemTime::now().duration_since(UNIX_EPOCH).map(|d|d.as_millis()).unwrap_or(0) as i64)}
/// "2026-11-02 15:23" local time (CET/CEST) of a millisecond timestamp, for dates in the UI.
/// Full local stamp "2026-11-02 15:23:45.123".
pub fn local_stamp(ms:i64)->String{stamp_at(ms).0}
pub fn local_time(ms:i64)->String{let t=stamp_at(ms).0;t.get(..16).unwrap_or(&t).to_string()}
fn stamp_at(ms:i64)->(String,String){
    let secs=ms/1000;
    let civil=|s:i64|{let z=s.div_euclid(86400)+719468;let rem=s.rem_euclid(86400);let era=z.div_euclid(146097);let doe=z-era*146097;
        let yoe=(doe-doe/1460+doe/36524-doe/146096)/365;let doy=doe-(365*yoe+yoe/4-yoe/100);let mp=(5*doy+2)/153;let d=doy-(153*mp+2)/5+1;
        let m=if mp<10{mp+3}else{mp-9};(yoe+era*400+if m<=2{1}else{0},m,d,rem/3600,rem%3600/60,rem%60)};
    let year=civil(secs).0;
    let days=|y:i64,m:i64,d:i64|{let y=if m<=2{y-1}else{y};let era=y.div_euclid(400);let yoe=y-era*400;let doy=(153*(m+if m>2{-3}else{9})+2)/5+d-1;era*146097+yoe*365+yoe/4-yoe/100+doy-719468};
    let last_sunday=|month:i64|{let d=days(year,month,31);d-(d+4).rem_euclid(7)};
    let summer=secs>=last_sunday(3)*86400+3600 && secs<last_sunday(10)*86400+3600;
    let (y,mo,d,h,mi,s)=civil(secs+if summer{7200}else{3600});
    (format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}.{:03}",ms.rem_euclid(1000)),format!("{y:04}{mo:02}{d:02}"))
}

/// Start writing to `dir` (created when needed). Without init the log only keeps the in-memory tail.
pub fn init(dir:PathBuf){let _=std::fs::create_dir_all(&dir);log().lock().unwrap().dir=Some(dir);}
pub fn dir()->Option<PathBuf>{log().lock().unwrap().dir.clone()}

/// Extra destination for every line (the SQLite `log` index); the day file stays the primary record.
pub type Sink=Box<dyn Fn(&Line)+Send+Sync>;
fn sink()->&'static Mutex<Option<Sink>>{static S:OnceLock<Mutex<Option<Sink>>>=OnceLock::new();S.get_or_init(||Mutex::new(None))}
pub fn set_sink(s:Option<Sink>){*sink().lock().unwrap()=s;}

pub fn write(level:Level,area:&str,text:&str){
    if let Some(line)=write_local(level,area,text){if let Some(s)=sink().lock().unwrap().as_ref(){s(&line);}}
}
/// Day file and tail only, never the sink (used by the sink itself to report its own errors).
pub fn write_local(level:Level,area:&str,text:&str)->Option<Line>{
    let (time,day)=stamp();
    let text=text.trim_end().replace('\n'," | ");
    let line=Line{time,level,area:area.to_string(),text};
    let mut l=log().lock().unwrap();
    if let Some(d)=&l.dir{
        if let Ok(mut f)=std::fs::OpenOptions::new().create(true).append(true).open(d.join(format!("pumpstudio_{day}.log"))){let _=writeln!(f,"{}",line.render());}
    }
    l.seq+=1;l.tail.push(line.clone());
    if l.tail.len()>TAIL{let n=l.tail.len()-TAIL;l.tail.drain(..n);}
    Some(line)
}
pub fn info(area:&str,text:&str){write(Level::Info,area,text)}
pub fn warn(area:&str,text:&str){write(Level::Warn,area,text)}
pub fn error(area:&str,text:&str){write(Level::Error,area,text)}
/// (sequence number, copy of the tail) so the UI only copies when something changed.
pub fn snapshot(since:u64)->Option<(u64,Vec<Line>)>{let l=log().lock().unwrap();if l.seq==since{None}else{Some((l.seq,l.tail.clone()))}}

#[cfg(test)] mod tests{
    use super::*;
    #[test]fn writes_daily_file_and_tail(){
        let dir=std::env::temp_dir().join(format!("pumpstudio-log-{}",std::process::id()));init(dir.clone());
        warn("CAN","bus fout\nregel 2");
        // Other tests log concurrently into the same global tail: look the line up instead of taking the last one.
        let (_,tail)=snapshot(0).unwrap();let line=tail.iter().rev().find(|l|l.text.starts_with("bus fout")).unwrap();
        assert_eq!((line.level,line.area.as_str(),line.text.as_str()),(Level::Warn,"CAN","bus fout | regel 2"));
        let file=std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
        assert!(std::fs::read_to_string(file).unwrap().contains("WARN  CAN        bus fout"));
    }
}
