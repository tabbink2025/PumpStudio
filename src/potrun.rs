//! Pot executor. Every head runs on its own, in its own rhythm (L3): start (unit Start, lift up with
//! P1 = height × 10) → tare (stable weight) → per step: one CAN block for the items of that step → weigh →
//! [one top-up block → weigh] → end (lift down, unit Stop). A slow head (long step, top-up, stable wait) never
//! holds up the other head, and a fault on one head (drive error, no pot, PLC reply) only fails that head.
//! - `run_pot`: iteration 4, one pot under the heads one after the other ("Volgende kop" in between).
//! - `run_heads`: each head on its own pot at the same time, one thread per head (iteration 5).
//! CAN blocks of both heads are serialised by the CAN worker; weighing, top-up and PLC waits are per head.
//! STOP stores what was weighed and lowers the lift. Hardware access goes through `PotIo` (a fake in the tests).
use crate::{machine::Head,model::Result,recipe::{self,PlanItem,PlanStep,PotItemRow,PotPlan}};

/// One pump of a CAN block as it ran: steps sent and start/end (ms since epoch).
#[derive(Clone,Debug,PartialEq)]
pub struct BlockDose{pub pump:String,pub steps:i64,pub start_ms:i64,pub end_ms:i64}
/// Hardware of one head (implemented on the device workers; a fake in the tests).
pub trait PotIo:Send{
    /// Lift up (Start, P1 = height × 10) / down (Stop). `force` = also after STOP (lowering is always allowed).
    /// Err = this head fails, with the reason (e.g. "geen pot (PLC-antwoord 16)").
    fn lift(&mut self,head:Head,up:bool,p1:u32,force:bool)->Result<()>;
    fn unit(&mut self,head:Head,start:bool,force:bool)->Result<()>;
    /// Stable gross weight of this head's scale.
    fn weigh(&mut self,head:Head)->Result<f64>;
    /// One CAN block for this head: all items (pump, grams, calibration id) start together.
    fn dose(&mut self,head:Head,items:&[(String,f64,i64)])->Result<Vec<BlockDose>>;
    fn stopped(&self)->bool;
    fn store(&mut self,item:&PotItemRow)->Result<()>;
    /// STOP / error: unit Stop without waiting for its reply (Camelot's Abort path). A normal end uses `unit` and
    /// waits for 9.
    fn unit_stop_now(&mut self,head:Head)->Result<()>{self.unit(head,false,true)}
    /// Store without any wait (no busy wait, no new connection); false = not stored yet, `store` it later.
    fn store_now(&mut self,item:&PotItemRow)->bool{self.store(item).is_ok()}
    /// Operator step (e.g. "Zet de pot onder LH2"); false when stopped.
    fn wait_operator(&mut self,message:&str)->bool;
    fn progress(&mut self,head:Option<Head>,text:&str);
}

/// What one head does: its steps of the plan, on pot run `run`, lift height as P1 (height in cm × 10).
#[derive(Clone,Debug)]
pub struct HeadJob{pub head:Head,pub run:i64,pub steps:Vec<PlanStep>,pub p1:u32}
/// Outcome of a pot run or one head: "klaar", "gestopt" or "fout" and a note.
#[derive(Clone,Debug,PartialEq)]
pub struct PotOutcome{pub status:&'static str,pub note:String}

#[derive(Clone,Debug)]
struct Item{plan:PlanItem,dosed:Option<f64>,topped:bool,topup_g:f64,block:Option<BlockDose>,topup_block:Option<BlockDose>}
enum Halt{Stop,Fail(String)}
fn halt(io:&dyn PotIo,e:String)->Halt{if io.stopped(){Halt::Stop}else{Halt::Fail(e)}}
fn halt_status(h:&Halt)->&'static str{match h{Halt::Stop=>"gestopt",Halt::Fail(_)=>"fout"}}

/// One pot under the heads one after the other (iteration 4, manual): after a head is done the operator puts
/// the pot under the next head and clicks "Volgende kop". A head that fails or is stopped ends the pot.
pub fn run_pot(io:&mut dyn PotIo,run:i64,plan:&PotPlan,heads:&[Head],p1:[u32;2])->PotOutcome{
    let mut notes=vec![];
    for (g,h) in heads.iter().enumerate(){
        if g>0{
            let msg=format!("Zet de pot onder {} en klik Volgende kop",h.name());
            io.progress(None,&msg);
            if !io.wait_operator(&msg){notes.push("Gestopt (STOP) vóór volgende kop".to_string());return PotOutcome{status:recipe::POT_STOPPED,note:notes.join("; ")};}
        }
        let out=guarded_head(io,&HeadJob{head:*h,run,steps:plan.part(*h),p1:p1[h.idx()]});
        if !out.note.is_empty(){notes.push(out.note.clone());}
        if out.status!=recipe::POT_DONE{return PotOutcome{status:out.status,note:notes.join("; ")};}
    }
    PotOutcome{status:recipe::POT_DONE,note:notes.join("; ")}
}

/// Every head on its own pot at the same time, one thread per head; returns the outcome per head.
pub fn run_heads(jobs:Vec<(HeadJob,Box<dyn PotIo>)>)->Vec<(Head,PotOutcome)>{
    std::thread::scope(|s|{
        let hs:Vec<_>=jobs.into_iter().map(|(job,mut io)|(job.head,s.spawn(move||guarded_head(io.as_mut(),&job)))).collect();
        hs.into_iter().map(|(h,t)|(h,t.join().unwrap_or(PotOutcome{status:recipe::POT_FAILED,note:format!("{}: kop-taak afgebroken",h.name())}))).collect()
    })
}

/// `run_head` with a guard: a panic in this head's work (a bug) still lowers this head's lift and stops its unit,
/// and comes back as "fout" naming the head, instead of leaving the lift up.
pub fn guarded_head(io:&mut dyn PotIo,job:&HeadJob)->PotOutcome{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(||run_head(&mut *io,job))){
        Ok(o)=>o,
        Err(p)=>{
            let h=job.head;let why=p.downcast_ref::<&str>().map(|s|s.to_string()).or_else(||p.downcast_ref::<String>().cloned()).unwrap_or_else(||"onbekend".into());
            crate::applog::error("RECEPT",&format!("{}: interne fout (paniek): {why}; lift omlaag",h.name()));
            let r=std::panic::catch_unwind(std::panic::AssertUnwindSafe(||{let _=io.lift(h,false,0,true);let _=io.unit_stop_now(h);io.progress(Some(h),&format!("{}: interne fout, lift omlaag",h.name()));}));
            PotOutcome{status:recipe::POT_FAILED,note:format!("{}: interne fout (paniek): {why}{}",h.name(),if r.is_err(){"; lift omlaag mislukt"}else{""})}
        }
    }
}
/// One head from lift up to lift down.
pub fn run_head(io:&mut dyn PotIo,job:&HeadJob)->PotOutcome{
    let h=job.head;let mut moved=false;
    // The step interrupted by STOP/error is stored only after the lift is down (no database write before the lift).
    let mut stash:Option<(Vec<Item>,&'static str)>=None;
    let r=body(io,job,&mut moved,&mut stash);
    let mut out=match &r{
        Ok(())=>PotOutcome{status:recipe::POT_DONE,note:String::new()},
        Err(Halt::Stop)=>PotOutcome{status:recipe::POT_STOPPED,note:format!("{}: gestopt (STOP)",h.name())},
        Err(Halt::Fail(e))=>PotOutcome{status:recipe::POT_FAILED,note:format!("{}: {e}",h.name())},
    };
    // Interrupted step: stored as it is now with a write that never waits (so a crash after the lift loses nothing);
    // whatever could not be written at once is stored after the lift is down. The lift never waits for the database.
    let mut later=vec![];let mut texts=vec![];
    if let Some((mut items,st))=stash.take(){for (row,t) in step_rows(h,job.run,&mut items,st){if !io.store_now(&row){later.push(row);}texts.push(t);}}
    if moved{
        // Lowering is always allowed and comes first: at the end, after STOP and after an error (also "geen pot").
        if let Err(e)=io.lift(h,false,0,true){
            io.progress(Some(h),&format!("{}: lift omlaag mislukt: {e}",h.name()));
            if out.status==recipe::POT_DONE{out=PotOutcome{status:recipe::POT_FAILED,note:format!("{}: lift omlaag mislukt: {e}",h.name())};}
        }
        if r.is_ok(){
            // Normal end (Camelot UnitModule): wait for unit Stop 9; anything else fails this head.
            if let Err(e)=io.unit(h,false,true){
                io.progress(Some(h),&format!("{}: unit stop mislukt: {e}",h.name()));
                if out.status==recipe::POT_DONE{out=PotOutcome{status:recipe::POT_FAILED,note:format!("{}: unit stop: {e}",h.name())};}
            }
        }else if let Err(e)=io.unit_stop_now(h){io.progress(Some(h),&format!("{}: unit stop mislukt: {e}",h.name()));}
    }
    if let Err(x)=&r{io.progress(Some(h),&format!("{}: {}; lift omlaag",h.name(),match x{Halt::Stop=>"gestopt (STOP)".to_string(),Halt::Fail(e)=>format!("fout: {e}")}));}
    else if moved{io.progress(Some(h),&format!("{}: lift omlaag, unit stop",h.name()));}
    for t in texts{io.progress(Some(h),&t);}
    for row in later{if let Err(e)=io.store(&row){io.progress(Some(h),&format!("Niet opgeslagen: {e}"));}}
    out
}
fn body(io:&mut dyn PotIo,job:&HeadJob,moved:&mut bool,stash:&mut Option<(Vec<Item>,&'static str)>)->std::result::Result<(),Halt>{
    let h=job.head;
    if io.stopped(){return Err(Halt::Stop);}
    if job.steps.is_empty(){return Ok(());}
    io.progress(Some(h),&format!("{}: unit start, lift omhoog (P1 = {})",h.name(),job.p1));
    *moved=true;
    if let Err(e)=io.unit(h,true,false){return Err(halt(io,e));}
    if let Err(e)=io.lift(h,true,job.p1,false){return Err(halt(io,e));}
    let mut base=weigh(io,h)?;
    for s in &job.steps{step(io,job.run,h,s,&mut base,stash)?;}
    Ok(())
}
fn weigh(io:&mut dyn PotIo,h:Head)->std::result::Result<f64,Halt>{
    if io.stopped(){return Err(Halt::Stop);}
    match io.weigh(h){Ok(w)=>Ok(w),Err(e)=>Err(halt(io,e))}
}
/// One step of one head: block → weigh → [top-up block → weigh] → store. On STOP/error the step is stored
/// with what was measured (not weighed = no value) and the halt is passed up.
fn step(io:&mut dyn PotIo,run:i64,h:Head,s:&PlanStep,base:&mut f64,stash:&mut Option<(Vec<Item>,&'static str)>)->std::result::Result<(),Halt>{
    let mut items:Vec<Item>=s.items.iter().map(|p|Item{plan:p.clone(),dosed:None,topped:false,topup_g:0.,block:None,topup_block:None}).collect();
    if io.stopped(){*stash=Some((items,"gestopt"));return Err(Halt::Stop);}
    let list:Vec<(String,f64,i64)>=items.iter().map(|x|(x.plan.pump.clone(),x.plan.grams,x.plan.calibration_id)).collect();
    io.progress(Some(h),&format!("{}: doseren stap {}: {}",h.name(),s.idx+1,list.iter().map(|(p,g,_)|format!("{p} {} g",crate::pumptest::nl(*g,3))).collect::<Vec<_>>().join(", ")));
    match io.dose(h,&list){
        Ok(b)=>for x in items.iter_mut(){x.block=b.iter().find(|b|b.pump==x.plan.pump).cloned();}
        Err(e)=>{let st=halt(io,e);*stash=Some((items,halt_status(&st)));return Err(st);}
    }
    let w=match weigh(io,h){Ok(w)=>w,Err(st)=>{*stash=Some((items,halt_status(&st)));return Err(st);}};
    let net=w-*base;*base=w;
    let req:Vec<f64>=items.iter().map(|x|x.plan.grams).collect();
    for (x,share) in items.iter_mut().zip(recipe::attribute(net,&req)){x.dosed=Some(share);}
    // Top-up once, only below the lower limit and only when the step allows it.
    let low:Vec<usize>=(0..items.len()).filter(|j|{let x=&items[*j];recipe::judge(x.plan.grams,x.dosed.unwrap_or(0.),x.plan.tolerance_pct)=="te_weinig"}).collect();
    if s.topup && !low.is_empty(){
        for j in &low{let x=&mut items[*j];x.topup_g=x.plan.grams-x.dosed.unwrap_or(0.);}
        let list:Vec<(String,f64,i64)>=low.iter().map(|j|{let x=&items[*j];(x.plan.pump.clone(),x.topup_g,x.plan.calibration_id)}).collect();
        io.progress(Some(h),&format!("{}: bijdoseren stap {}: {}",h.name(),s.idx+1,list.iter().map(|(p,g,_)|format!("{p} {} g",crate::pumptest::nl(*g,4))).collect::<Vec<_>>().join(", ")));
        // Values weighed before the top-up stay as measured if the top-up is stopped or fails.
        match io.dose(h,&list){
            Ok(b)=>for j in &low{let x=&mut items[*j];x.topup_block=b.iter().find(|b|b.pump==x.plan.pump).cloned();}
            Err(e)=>{let st=halt(io,e);*stash=Some((items,halt_status(&st)));return Err(st);}
        }
        let w=match weigh(io,h){Ok(w)=>w,Err(st)=>{*stash=Some((items,halt_status(&st)));return Err(st);}};
        let net=w-*base;*base=w;
        let req:Vec<f64>=low.iter().map(|j|items[*j].topup_g).collect();
        for (j,share) in low.iter().zip(recipe::attribute(net,&req)){let x=&mut items[*j];x.dosed=Some(x.dosed.unwrap_or(0.)+share);x.topped=true;}
    }
    store_step(io,run,h,&mut items,"");
    Ok(())
}
/// Store the items of a step (judged against their tolerance; parallel items flagged as attributed).
/// `status_override` ("gestopt"/"fout") for an interrupted step: values only as far as they were weighed.
fn store_step(io:&mut dyn PotIo,run:i64,h:Head,items:&mut Vec<Item>,status_override:&str){
    for (row,t) in step_rows(h,run,items,status_override){
        io.progress(Some(h),&t);
        if let Err(e)=io.store(&row){io.progress(Some(h),&format!("Niet opgeslagen: {e}"));}
    }
}
/// The rows of a step to store, with their progress line.
fn step_rows(h:Head,run:i64,items:&mut Vec<Item>,status_override:&str)->Vec<(PotItemRow,String)>{
    let parallel=items.len()>1;let mut out=vec![];
    for x in std::mem::take(items){
        let status=if !status_override.is_empty(){status_override.to_string()}else{match x.dosed{Some(d)=>recipe::judge(x.plan.grams,d,x.plan.tolerance_pct).into(),None=>"fout".into()}};
        let b=x.block.clone();
        let mut note=vec![];if parallel && x.dosed.is_some(){note.push("parallel: aandeel van het nettogewicht van de kop naar verhouding".to_string());}
        if x.topped{note.push(format!("bijgedoseerd {} g",crate::pumptest::nl(x.topup_g,3)));}
        else if x.topup_g>0. && x.dosed.is_some(){note.push("bijdoseren niet afgerond: waarde van vóór het bijdoseren".into());}
        let row=PotItemRow{pot_run_id:run,step_idx:x.plan.step as i64,item_idx:x.plan.item as i64,ingredient_id:Some(x.plan.ingredient_id),ingredient:x.plan.ingredient.clone(),
            pump:x.plan.pump.clone(),head:h.name().into(),requested_g:x.plan.grams,dosed_g:x.dosed,attributed:parallel && x.dosed.is_some(),topped_up:x.topped,
            deviation_pct:x.dosed.map(|d|100.*(d-x.plan.grams)/x.plan.grams),tolerance_pct:x.plan.tolerance_pct,status:status.clone(),calibration_id:Some(x.plan.calibration_id),
            steps:b.as_ref().map(|b|b.steps+x.topup_block.as_ref().map(|t|t.steps).unwrap_or(0)),started_ms:b.as_ref().map(|b|b.start_ms),
            ended_ms:x.topup_block.as_ref().or(b.as_ref()).map(|b|b.end_ms),note:note.join("; "),..Default::default()};
        let t=format!("{}: {} {} g gevraagd, {} gedoseerd — {}",h.name(),row.ingredient,crate::pumptest::nl(row.requested_g,3),
            row.dosed_g.map(|d|format!("{} g",crate::pumptest::nl(d,4))).unwrap_or("niet gewogen".into()),recipe::status_label(&status));
        out.push((row,t));
    }
    out
}

#[cfg(test)] mod tests{
    use super::*;
    use crate::recipe::PlanItem;
    use std::{collections::HashMap,sync::{Arc,Mutex},time::{Duration,Instant}};
    /// Fake machine shared by the heads: each pump delivers `gain` × requested grams, scales sum per head.
    /// Optional delays per pump (dose) and per head (stable weighing), drive errors per pump, STOP after n blocks.
    /// CAN blocks are serialised (like the CAN worker); weighings are per head.
    #[derive(Default)]
    struct St{gain:HashMap<String,f64>,weight:[f64;2],log:Vec<String>,events:Vec<(String,Instant)>,stored:Vec<PotItemRow>,stop_after_doses:Option<usize>,doses:usize,
        lift:[bool;2],t:i64,delay:HashMap<String,u64>,weigh_ms:[u64;2],bad:HashMap<String,String>,lift_reply:[Option<String>;2],unit_stop_err:bool}
    #[derive(Clone)]
    struct Fake{st:Arc<Mutex<St>>,can:Arc<Mutex<()>>}
    impl Fake{
        fn new(g:&[(&str,f64)])->Self{Self{st:Arc::new(Mutex::new(St{gain:g.iter().map(|(p,x)|(p.to_string(),*x)).collect(),weight:[100.,200.],..Default::default()})),can:Arc::new(Mutex::new(()))}}
        fn s(&self)->std::sync::MutexGuard<'_,St>{self.st.lock().unwrap()}
        fn log(&self,l:String){let mut s=self.s();s.events.push((l.clone(),Instant::now()));s.log.push(l);}
    }
    impl PotIo for Fake{
        fn lift(&mut self,h:Head,up:bool,p1:u32,force:bool)->Result<()>{
            if self.stopped() && !force{return Err("Gestopt".into());}
            let reply=self.s().lift_reply[h.idx()].clone();
            if up{if let Some(e)=reply{self.log(format!("lift {} op {p1}: {e}",h.name()));return Err(e);}}
            self.s().lift[h.idx()]=up;self.log(if up{format!("lift {} op {p1}",h.name())}else{format!("lift {} neer",h.name())});Ok(())
        }
        fn unit(&mut self,h:Head,start:bool,_f:bool)->Result<()>{self.log(format!("unit {} {}",h.name(),if start{"start"}else{"stop"}));
            if !start && self.s().unit_stop_err{return Err("onverwacht PLC-antwoord 12 op unit stop".into());}Ok(())}
        fn unit_stop_now(&mut self,h:Head)->Result<()>{self.log(format!("unit {} stop (zonder wachten)",h.name()));Ok(())}
        fn weigh(&mut self,h:Head)->Result<f64>{let ms=self.s().weigh_ms[h.idx()];std::thread::sleep(Duration::from_millis(ms));Ok(self.s().weight[h.idx()])}
        fn dose(&mut self,h:Head,items:&[(String,f64,i64)])->Result<Vec<BlockDose>>{
            let can=self.can.clone();let _bus=can.lock().unwrap();
            {let mut s=self.s();s.doses+=1;if s.stop_after_doses.is_some_and(|n|s.doses>n){return Err("Gestopt".into());}
                if let Some(e)=items.iter().find_map(|x|s.bad.get(&x.0).cloned()){drop(s);self.log(format!("dose {} fout",h.name()));return Err(e);}}
            let ms=items.iter().map(|x|self.s().delay.get(&x.0).copied().unwrap_or(0)).max().unwrap_or(0);std::thread::sleep(Duration::from_millis(ms));
            let mut s=self.s();s.t+=10;let t=s.t;let mut out=vec![];
            for (p,g,_) in items{let gain=s.gain.get(p).copied().unwrap_or(1.);s.weight[h.idx()]+=g*gain;out.push(BlockDose{pump:p.clone(),steps:(g*1000.) as i64,start_ms:t,end_ms:t+5});}
            drop(s);self.log(format!("dose {}",items.iter().map(|x|x.0.clone()).collect::<Vec<_>>().join("+")));Ok(out)
        }
        fn stopped(&self)->bool{let s=self.s();s.stop_after_doses.is_some_and(|n|s.doses>n)}
        fn store(&mut self,i:&PotItemRow)->Result<()>{self.s().stored.push(i.clone());Ok(())}
        fn wait_operator(&mut self,m:&str)->bool{self.log(format!("operator: {m}"));!self.stopped()}
        fn progress(&mut self,_h:Option<Head>,_t:&str){}
    }
    fn it(step:usize,item:usize,name:&str,g:f64,pump:&str,head:Head)->PlanItem{PlanItem{step,item,ingredient_id:1,ingredient:name.into(),grams:g,tolerance_pct:1.,pump:pump.into(),head,calibration_id:1}}
    fn r1()->PotPlan{PotPlan{version_id:1,recipe:"R1".into(),version:1,pot_height_cm:None,steps:vec![
        PlanStep{idx:0,topup:false,items:vec![it(0,0,"A",1.,"P_C01",Head::LH1)]},
        PlanStep{idx:1,topup:false,items:vec![it(1,0,"B",2.,"P_C02",Head::LH1),it(1,1,"C",1.,"P_C03",Head::LH1)]},
        PlanStep{idx:2,topup:false,items:vec![it(2,0,"D",0.5,"P_C04",Head::LH1),it(2,1,"E",1.5,"P_A01",Head::LH2)]},
        PlanStep{idx:3,topup:false,items:vec![it(3,0,"F",3.,"P_A02",Head::LH2)]}]}}
    #[test]fn r1_manual_one_pot_order_heads_and_parallel_block(){
        let mut f=Fake::new(&[("P_C02",1.01),("P_C03",1.01)]);
        let out=run_pot(&mut f,7,&r1(),&[Head::LH1,Head::LH2],[150,95]);assert_eq!(out.status,"klaar","{out:?}");
        assert_eq!(f.s().log,vec!["unit LH1 start","lift LH1 op 150","dose P_C01","dose P_C02+P_C03","dose P_C04","lift LH1 neer","unit LH1 stop",
            "operator: Zet de pot onder LH2 en klik Volgende kop","unit LH2 start","lift LH2 op 95","dose P_A01","dose P_A02","lift LH2 neer","unit LH2 stop"]);
        let s=f.s().stored.clone();assert_eq!(s.iter().map(|x|x.ingredient.as_str()).collect::<Vec<_>>(),vec!["A","B","C","D","E","F"]);
        // B and C: one block (same start), net 3,03 g shared 2:1 → 2,02 / 1,01, flagged.
        assert_eq!(s[1].started_ms,s[2].started_ms);assert!(s[1].attributed && s[2].attributed && !s[0].attributed);
        assert!((s[1].dosed_g.unwrap()-2.02).abs()<1e-9 && (s[2].dosed_g.unwrap()-1.01).abs()<1e-9);assert!(s[1].note.contains("naar verhouding"));
        assert!(s.iter().all(|x|x.status=="ok"));assert_eq!(f.s().lift,[false,false]);
    }
    #[test]fn normal_end_waits_for_unit_stop_and_fails_on_a_wrong_reply(){
        let mut plan=r1();plan.steps.truncate(1);
        // Normal end: lift down, then unit Stop with its reply (not the no-wait path).
        let mut f=Fake::new(&[]);assert_eq!(run_pot(&mut f,1,&plan,&[Head::LH1],[100,100]).status,"klaar");
        let l=f.s().log.clone();assert_eq!(&l[l.len()-2..],&["lift LH1 neer","unit LH1 stop"]);
        // Unit Stop answers something else than 9: the head fails (pot "fout"), the dosed values stay stored.
        let mut f=Fake::new(&[]);f.s().unit_stop_err=true;let out=run_pot(&mut f,1,&plan,&[Head::LH1],[100,100]);
        assert_eq!(out.status,"fout");assert!(out.note.contains("unit stop"),"{out:?}");assert_eq!(f.s().stored.len(),1);
        // STOP: the interrupted step is written without waiting before the lift goes down; unit Stop without waiting.
        let mut f=Fake::new(&[]);f.s().stop_after_doses=Some(0);let out=run_pot(&mut f,1,&plan,&[Head::LH1],[100,100]);
        assert_eq!(out.status,"gestopt");let l=f.s().log.clone();assert_eq!(&l[l.len()-2..],&["lift LH1 neer","unit LH1 stop (zonder wachten)"]);
        assert_eq!(f.s().stored[0].status,"gestopt");
    }
    #[test]fn r4_top_up_once_when_low_overdose_reported(){
        let mut plan=r1();plan.steps.truncate(1);plan.steps[0].topup=true;
        let doses=|f:&Fake|f.s().log.iter().filter(|l|l.starts_with("dose")).count();
        // 2 % too little → top-up of the missing 0,02 g (pump still 2 % short → 0,9996 g, within ±1 %).
        let mut f=Fake::new(&[("P_C01",0.98)]);run_pot(&mut f,1,&plan,&[Head::LH1],[100,100]);
        let x=f.s().stored[0].clone();assert!(x.topped_up && x.status=="ok","{x:?}");assert!((x.dosed_g.unwrap()-(0.98+0.02*0.98)).abs()<1e-9);
        assert_eq!(doses(&f),2,"maximaal één keer bijdoseren");
        // Still far too little after one top-up: reported, no second top-up.
        let mut f=Fake::new(&[("P_C01",0.5)]);run_pot(&mut f,1,&plan,&[Head::LH1],[100,100]);
        let x=f.s().stored[0].clone();assert_eq!((x.status.as_str(),x.topped_up),("te_weinig",true));assert_eq!(doses(&f),2);
        // Top-up off: too little reported, not corrected.
        plan.steps[0].topup=false;let mut f=Fake::new(&[("P_C01",0.98)]);run_pot(&mut f,1,&plan,&[Head::LH1],[100,100]);
        let x=f.s().stored[0].clone();assert_eq!((x.status.as_str(),x.topped_up),("te_weinig",false));
        // Too much: reported, never corrected, value kept as measured.
        plan.steps[0].topup=true;let mut f=Fake::new(&[("P_C01",1.03)]);run_pot(&mut f,1,&plan,&[Head::LH1],[100,100]);
        let x=f.s().stored[0].clone();assert_eq!((x.status.as_str(),x.topped_up),("te_veel",false));assert!((x.dosed_g.unwrap()-1.03).abs()<1e-9,"gemeten waarde blijft");
        assert_eq!(doses(&f),1);
    }
    #[test]fn r5_stop_lowers_lift_and_keeps_what_was_dosed(){
        let mut f=Fake::new(&[]);f.s().stop_after_doses=Some(2);
        let out=run_pot(&mut f,1,&r1(),&[Head::LH1,Head::LH2],[100,100]);assert_eq!(out.status,"gestopt");
        assert_eq!(f.s().lift,[false,false],"lift omlaag na STOP");assert!(f.s().log.contains(&"lift LH1 neer".to_string()));
        let s=f.s().stored.clone();assert_eq!(s.iter().filter(|x|x.status=="ok").count(),3,"A, B, C gedoseerd en bewaard");
        assert!(s.iter().any(|x|x.ingredient=="D" && x.status=="gestopt" && x.dosed_g.is_none()),"{s:?}");
        assert!(!f.s().log.iter().any(|l|l.contains("LH2 op")),"LH2 niet meer gestart");
    }
    #[test]fn panic_in_a_head_lowers_its_lift_and_names_the_head(){
        struct Boom(Fake);
        impl PotIo for Boom{
            fn lift(&mut self,h:Head,up:bool,p1:u32,f:bool)->Result<()>{self.0.lift(h,up,p1,f)}
            fn unit(&mut self,h:Head,s:bool,f:bool)->Result<()>{self.0.unit(h,s,f)}
            fn unit_stop_now(&mut self,h:Head)->Result<()>{self.0.unit_stop_now(h)}
            fn weigh(&mut self,h:Head)->Result<f64>{if h==Head::LH1{panic!("weegfout in test")}self.0.weigh(h)}
            fn dose(&mut self,h:Head,i:&[(String,f64,i64)])->Result<Vec<BlockDose>>{self.0.dose(h,i)}
            fn stopped(&self)->bool{self.0.stopped()}
            fn store(&mut self,i:&PotItemRow)->Result<()>{self.0.store(i)}
            fn wait_operator(&mut self,m:&str)->bool{self.0.wait_operator(m)}
            fn progress(&mut self,h:Option<Head>,t:&str){self.0.progress(h,t)}
        }
        let f=Fake::new(&[]);let (a,b)=two_pots();
        let out:HashMap<Head,PotOutcome>=run_heads(vec![(a,Box::new(Boom(f.clone()))),(b,Box::new(Boom(f.clone())))]).into_iter().collect();
        assert_eq!(out[&Head::LH1].status,"fout");assert!(out[&Head::LH1].note.starts_with("LH1: interne fout"),"{:?}",out[&Head::LH1]);
        assert_eq!(out[&Head::LH2].status,"klaar","andere kop gaat door");
        assert_eq!(f.s().lift,[false,false],"lift LH1 omlaag na paniek");assert!(f.s().log.contains(&"unit LH1 stop (zonder wachten)".to_string()));
    }
    #[test]fn l2_no_pot_reply_fails_head_without_dosing_and_lowers_lift(){
        let mut f=Fake::new(&[]);f.s().lift_reply[0]=Some("geen pot (PLC-antwoord 16)".into());
        let out=run_pot(&mut f,1,&r1(),&[Head::LH1,Head::LH2],[100,100]);
        assert_eq!(out.status,"fout");assert!(out.note.contains("LH1: geen pot"),"{out:?}");
        let log=f.s().log.clone();assert!(!log.iter().any(|l|l.starts_with("dose")),"niet gedoseerd: {log:?}");
        assert!(log.contains(&"lift LH1 neer".to_string()) && log.contains(&"unit LH1 stop (zonder wachten)".to_string()),"{log:?}");
        assert!(!log.iter().any(|l|l.contains("LH2")),"pot niet naar LH2");assert!(f.s().stored.is_empty());
    }
    /// LH1 on pot 1, LH2 on pot 2. LH1: long block + top-up + slow stable weighing; LH2: three short steps.
    fn two_pots()->(HeadJob,HeadJob){
        let l1=vec![PlanStep{idx:0,topup:true,items:vec![it(0,0,"A",1.,"P_C01",Head::LH1)]}];
        let l2=(0..3).map(|k|PlanStep{idx:k,topup:false,items:vec![it(k,0,"E",0.5,"P_A01",Head::LH2)]}).collect();
        (HeadJob{head:Head::LH1,run:1,steps:l1,p1:100},HeadJob{head:Head::LH2,run:2,steps:l2,p1:120})
    }
    #[test]fn l3a_slow_head_does_not_hold_up_the_other(){
        let f=Fake::new(&[("P_C01",0.9)]);{let mut s=f.s();s.delay.insert("P_C01".into(),300);s.weigh_ms=[250,10];}
        let (a,b)=two_pots();let t=Instant::now();
        let out=run_heads(vec![(a,Box::new(f.clone())),(b,Box::new(f.clone()))]);
        assert!(out.iter().all(|(_,o)|o.status=="klaar"),"{out:?}");
        let ev=f.s().events.clone();let at=|l:&str|ev.iter().find(|e|e.0==l).map(|e|e.1.duration_since(t)).unwrap();
        let (end1,end2)=(at("lift LH1 neer"),at("lift LH2 neer"));
        // LH1 alone: 250 tare + 300 block + 250 weigh + 300 top-up + 250 weigh ≈ 1,35 s. LH2 must not wait for it
        // (it waits at most for one LH1 block on the shared CAN bus): done long before LH1.
        assert!(end2+Duration::from_millis(500)<end1,"LH2 {end2:?} vs LH1 {end1:?}");
        let st=f.s().stored.clone();assert_eq!(st.iter().filter(|x|x.pot_run_id==2).count(),3);assert!(st.iter().any(|x|x.pot_run_id==1 && x.topped_up));
        eprintln!("L3a: LH2 klaar na {end2:?}, LH1 (lang + bijdoseren) na {end1:?}");
    }
    #[test]fn l3b_drive_error_on_one_head_only_fails_that_head(){
        let f=Fake::new(&[]);f.s().bad.insert("P_C01".into(),"drive P_C01: fout 0x23".into());
        let (a,b)=two_pots();
        let out:HashMap<Head,PotOutcome>=run_heads(vec![(a,Box::new(f.clone())),(b,Box::new(f.clone()))]).into_iter().collect();
        assert_eq!(out[&Head::LH1].status,"fout");assert!(out[&Head::LH1].note.contains("drive P_C01"));
        assert_eq!(out[&Head::LH2].status,"klaar","{:?}",out[&Head::LH2]);
        let st=f.s().stored.clone();
        assert!(st.iter().any(|x|x.pot_run_id==1 && x.status=="fout" && x.dosed_g.is_none()));assert!(st.iter().filter(|x|x.pot_run_id==2).all(|x|x.status=="ok"));
        assert_eq!(f.s().lift,[false,false],"beide liften omlaag");
    }
}
