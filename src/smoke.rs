//! `--smoke-test <dir>`: offline check without window or hardware. Everything lands in `<dir>`.
//! 1. Ceramic bench workflow (calibration, curve, doses).
//! 2. New shell: database in `<dir>/app/data`, every page rendered in both profiles, all VSD devices in
//!    simulation, a pump command, live weight, CAN scan, PLC message, STOP during a long dose, pump edit
//!    persisted, profile switch, ceramic bench + scale with a forward move.
use crate::ui::{render_all_pages,switch_frames,style,App};
use pump_studio::{calib,recipe,vsdcal::CalSettings,pumptest::{self,Mode,TestPlan},testdb};
use pump_studio::{applog,db::{Db,ProfileKind},devices::{DevId,Link},machine::{Head,PumpOp},model::*,plc::CMD_START,transport::StopHandle,workflow::*};
use std::{path::{Path,PathBuf},time::{Duration,Instant}};

pub fn run(dir:&Path)->Result<String>{
    std::fs::create_dir_all(dir).map_err(|e|e.to_string())?;
    applog::init(dir.join("logs"));
    let mut report=vec![];
    engine_part(&dir.join("results"))?;report.push("pompbank-workflow: ok".to_string());
    report.extend(shell_part(&dir.join("app"))?);
    Ok(report.join("\n"))
}

fn engine_part(output:&Path)->Result<()>{
    let mut engine=Engine::new(output.to_path_buf(),StopHandle::default());
    engine.execute(Action::Connect{cfg:Config::default(),simulation:true,python:PathBuf::new(),repo:PathBuf::new(),bridge:PathBuf::new()})?;
    engine.execute(Action::Zero)?;
    engine.execute(Action::Calibrate{revolutions:1,repeats:3,prime:0})?;
    engine.execute(Action::Characterize{interval:Interval::Degrees(10.),repeats:5})?;
    engine.execute(Action::Dose{grams:vec![5.;10],correction:-1.})?;
    let rows=&engine.state.records;
    if rows.len()!=10 || rows.iter().any(|r|r.status!="completed" || (r.mass_g.unwrap_or_default()-4.95).abs()>0.001){return Err("Simulatieresultaat wijkt af".into());}
    engine.execute(Action::Disconnect)?;
    Ok(())
}

fn wait(app:&mut App,what:&str,secs:f64,f:impl Fn(&App)->bool)->Result<Duration>{
    let t=Instant::now();
    // Same order as a UI frame: take worker updates first, then look.
    loop{app.tick();if f(app){break;}if t.elapsed().as_secs_f64()>secs{return Err(format!("Time-out: {what}"));}std::thread::sleep(Duration::from_millis(10));}
    Ok(t.elapsed())
}
fn on(app:&App,id:DevId)->bool{app.dev.status(id).link==Link::On}

fn shell_part(root:&Path)->Result<Vec<String>>{
    let _=std::fs::remove_dir_all(root);
    let mut out=vec![];
    let mut app=App::open(root.to_path_buf())?;
    if app.pumps.len()!=192 || !pump_studio::db::same_as_default(&app.pumps){return Err("VSD-profiel niet gelijk aan Camelot-bedrading".into());}
    out.push(format!("database {}: schema {}, VSD 192 pompen",app.db.path().display(),app.db.schema_version()?));
    let frames=render_all_pages(&mut app)?;out.push(format!("{frames} frames: alle 11 pagina's in beide profielen getekend zonder apparaten"));
    // VSD in simulation, every device separately.
    app.dev.settings.sim_travel_s=30.;
    for id in DevId::VSD{app.dev.connect(id,true)?;}
    wait(&mut app,"VSD verbonden",10.,|a|DevId::VSD.iter().all(|d|on(a,*d)) && a.dev.status(DevId::Lh1).weight.is_some())?;
    let w0=app.dev.status(DevId::Lh1).weight.unwrap_or_default();
    app.dev.pumps(vec!["P_C01".into(),"P_A01".into()],PumpOp::Dispense,51200)?;
    let t=wait(&mut app,"pompopdracht",10.,|a|!a.dev.busy(DevId::Can))?;
    if !app.dev.status(DevId::Can).ok{return Err(format!("Pompopdracht: {}",app.dev.status(DevId::Can).text));}
    wait(&mut app,"gewicht LH1",3.,|a|a.dev.status(DevId::Lh1).weight.is_some_and(|w|(w-w0-0.5).abs()<1e-6))?;
    out.push(format!("pomp P_C01 + P_A01 1 omw in {:.2} s; LH1 +0,5 g live gezien",t.as_secs_f64()));
    let id=app.pumps.iter().find(|p|p.name=="P_C01").map(|p|p.id).unwrap();
    if app.pumps.iter().find(|p|p.id==id).and_then(|p|p.position)!=Some(0.){return Err("Positie P_C01 niet bijgewerkt".into());}
    app.dev.scan(true,1.)?;wait(&mut app,"scan",5.,|a|!a.dev.busy(DevId::Can))?;
    if !app.dev.status(DevId::Can).scan.contains("hartslag van 610"){return Err(format!("Scan: {}",app.dev.status(DevId::Can).scan));}
    app.dev.plc_command(1350,CMD_START,vec![0])?;wait(&mut app,"PLC",5.,|a|!a.dev.busy(DevId::Plc))?;
    if !app.dev.status(DevId::Plc).messages.iter().any(|m|m.contains("<- proces 1350")){return Err("PLC-antwoord niet in berichtenlijst".into());}
    out.push("CAN-scan (simulatie) en PLC-bericht proces 1350 gezien".into());
    // STOP during a long dose.
    app.dev.pumps(vec!["P_D10".into()],PumpOp::Dispense,20*51200)?;std::thread::sleep(Duration::from_millis(300));
    if !app.dev.busy(DevId::Can){return Err("Lange dosering liep niet".into());}
    app.stop();let t=wait(&mut app,"STOP",2.,|a|!a.dev.busy(DevId::Can))?;
    if t>Duration::from_secs(1){return Err(format!("STOP duurde {t:?}"));}
    out.push(format!("STOP tijdens dosering van 20 omw: gestopt na {} ms",t.as_millis()));
    // After STOP the next command works normally (no STOP left behind).
    app.dev.scan(true,1.)?;wait(&mut app,"scan na STOP",5.,|a|!a.dev.busy(DevId::Can))?;
    if !app.dev.status(DevId::Can).scan.contains("hartslag van 610"){return Err(format!("Scan na STOP: {}",app.dev.status(DevId::Can).scan));}
    out.push("scan na STOP werkt normaal".into());
    // K1: exact steps. P_C05 at a known position, +25 600 st relative, then absolute 25 600 st.
    app.dev.pumps(vec!["P_C05".into()],PumpOp::Home,0)?;wait(&mut app,"home",5.,|a|!a.dev.busy(DevId::Can))?;
    app.dev.pumps(vec!["P_C05".into()],PumpOp::Dispense,25600)?;wait(&mut app,"25 600 st",5.,|a|!a.dev.busy(DevId::Can))?;
    let m=app.dev.status(DevId::Can).moves.get("P_C05").cloned().ok_or("Geen bewegingsverslag P_C05")?;
    if (m.before,m.target,m.reached,m.phase)!=(Some(0),25600,25600,Some(25600)){return Err(format!("K1 doseren: {m:?}"));}
    app.dev.pumps(vec!["P_C05".into()],PumpOp::MoveTo,-12800)?;wait(&mut app,"terug",5.,|a|!a.dev.busy(DevId::Can))?;
    if app.dev.status(DevId::Can).positions.get("P_C05")!=Some(&-12800){return Err("K1 naar positie -12 800 st".into());}
    if app.dev.pumps(vec!["P_C05".into()],PumpOp::Dispense,-1).is_ok(){return Err("K1: negatieve dosering niet geweigerd".into());}
    out.push("K1: doseren 25 600 st -> p1 25 600, SETPOS fase 25 600; naar positie -12 800 st; negatief geweigerd".into());
    // K2: VSD calibration (short settings, fast simulated travel), stored; second one archives the first.
    app.dev.world.as_ref().ok_or("geen simulatie")?.lock().unwrap().max_travel_s=0.05;
    let set=CalSettings{phase1_samples:11,phase1_runs:1,phase2_runs:1,stroke_samples:vec![1,2,5],settle_s:0.,..Default::default()};
    for k in 0..2{
        app.dev.calibrate(vec!["P_C02".into()],set.clone())?;wait(&mut app,"kalibratie",60.,|a|!a.dev.busy(DevId::Can))?;
        if !app.dev.status(DevId::Can).ok{return Err(format!("K2 kalibratie {}: {}",k+1,app.dev.status(DevId::Can).text));}
    }
    let h=app.db.calibrations(ProfileKind::Vsd,"P_C02")?;
    if h.len()!=2 || h[0].status!=calib::ACTIVE || h[1].status!=calib::ARCHIVE{return Err(format!("K2 historie: {:?}",h.iter().map(|r|&r.status).collect::<Vec<_>>()));}
    if app.db.calibration_points(h[0].id)?.iter().filter(|p|p.phase==2 && p.run==0).count()!=3{return Err("K2: fase-2 punten ontbreken".into());}
    if !app.cal_dates.contains_key("P_C02"){return Err("K2: kalibratiestatus op Pompen niet bijgewerkt".into());}
    out.push(format!("K2: P_C02 gekalibreerd ({:.4} g/slag), tweede kalibratie maakt eerste archief",h[0].g_per_stroke.unwrap_or(0.)));
    // K7: control dose 1 g with the active calibration.
    app.dev.check_dose("P_C02",1.)?;wait(&mut app,"controle",10.,|a|!a.dev.busy(DevId::Can))?;
    let c=app.db.calibration(h[0].id)?;let err=c.check_error_pct.ok_or("K7: controle niet opgeslagen")?;
    if err.abs()>2.{return Err(format!("K7: afwijking {err} %"));}
    out.push(format!("K7: controle 1 g -> {:.4} g ({err:+.3} %), opgeslagen bij kalibratie",c.check_measured.unwrap_or(0.)));
    // T1: series 0,5 / 1 / 5 g × 3 with the active (simulation) calibration of P_C02, stored, statistics per weight.
    let plan=TestPlan{mode:Mode::Reeks,weights:vec![0.5,1.,5.],repeats:3,settle_s:0.,..Default::default()};
    app.dev.set_correction(0.)?;let run=app.dev.start_test(&mut app.db,"P_C02",&plan)?;
    wait(&mut app,"T1 test",30.,|a|a.dev.running_test().is_none())?;
    let r=app.db.test_run(run)?;let d=app.db.test_doses(run)?;
    if r.status!=testdb::RUN_DONE || d.len()!=9 || d.iter().any(|x|x.err_pct().is_none_or(|e|e.abs()>2.)){return Err(format!("T1: {} {} doses",r.status,d.len()));}
    let st=pumptest::summary(&d,1.);if st.len()!=3 || st.iter().any(|w|w.n!=3){return Err("T1: uitslag per gewicht".into());}
    out.push(format!("T1: reeks 0,5/1/5 g × 3 = 9 doses opgeslagen; gem. afwijking {} %",st.iter().map(|w|pumptest::nls(w.mean_err_pct,2)).collect::<Vec<_>>().join(" / ")));
    // T7: Word report and CSV of that test.
    let bytes=pump_studio::report::test_report(&[(r.clone(),d.clone())],None)?;
    let doc=pump_studio::report::save_test_report(&root.join("verslag"),&r.pump,&[run],&bytes)?;
    let csv=root.join("verslag").join("pomptest.csv");std::fs::write(&csv,pumptest::csv("smoke",&d,1.)).map_err(|e|e.to_string())?;
    out.push(format!("T7: verslag {} ({} kB) en CSV geschreven",doc.display(),bytes.len()/1024));
    // T3: correction between doses (pause after every dose), recorded per dose; the target follows it.
    let plan=TestPlan{mode:Mode::Enkel,weights:vec![1.],repeats:2,settle_s:0.,pause_each:true,..Default::default()};
    let run=app.dev.start_test(&mut app.db,"P_C02",&plan)?;
    wait(&mut app,"T3 pauze",10.,|a|a.dev.running_test().is_some_and(|t|t.waiting))?;
    app.dev.set_correction(0.3)?;app.dev.resume_test();wait(&mut app,"T3 klaar",10.,|a|a.dev.running_test().is_none())?;
    let d=app.db.test_doses(run)?;
    if d.len()!=2 || d[0].correction_pct!=0. || d[1].correction_pct!=0.3 || d[1].target_g!=Some(1.003){return Err(format!("T3: {:?}",d.iter().map(|x|(x.correction_pct,x.target_g)).collect::<Vec<_>>()));}
    out.push(format!("T3: correctie +0,3 % tussen doses: doel 1,000 → 1,003 g, per dosis vastgelegd; aanbevolen nu {}",pumptest::recommended_correction(&d,5).map(|c|format!("{} % (dosis {}–{})",pumptest::nls(c.0,1),c.1,c.2)).unwrap_or("—".into())));
    app.dev.set_correction(0.)?;
    // K3: STOP during a calibration with real travel time.
    app.dev.world.as_ref().ok_or("geen simulatie")?.lock().unwrap().max_travel_s=30.;
    app.dev.calibrate(vec!["P_C02".into()],set.clone())?;
    wait(&mut app,"kalibratie in fase 2",30.,|a|a.dev.status(DevId::Can).progress.as_ref().is_some_and(|p|p.step>=12))?;
    std::thread::sleep(Duration::from_millis(300));app.stop();let t=wait(&mut app,"STOP kalibratie",2.,|a|!a.dev.busy(DevId::Can))?;
    let h2=app.db.calibrations(ProfileKind::Vsd,"P_C02")?;
    if t>Duration::from_secs(1) || h2[0].status!=calib::ABORTED || app.db.active_calibration(ProfileKind::Vsd,"P_C02",calib::KIND_VSD)?.map(|r|r.id)!=Some(h[0].id){
        return Err(format!("K3: {t:?}, status {}",h2[0].status));}
    out.push(format!("K3: STOP tijdens kalibratie na {} ms gestopt, opgeslagen als afgebroken, actieve kalibratie onveranderd",t.as_millis()));
    // T4: STOP during a test (real travel time): stopped within 1 s, status gestopt, doses until then kept.
    app.dev.world.as_ref().ok_or("geen simulatie")?.lock().unwrap().max_travel_s=0.05;
    app.dev.calibrate(vec!["P_C04".into()],set.clone())?;
    wait(&mut app,"kalibratie P_C04",60.,|a|!a.dev.busy(DevId::Can))?;
    app.dev.world.as_ref().ok_or("geen simulatie")?.lock().unwrap().max_travel_s=30.;
    let run=app.dev.start_test(&mut app.db,"P_C04",&TestPlan{mode:Mode::Enkel,weights:vec![5.],repeats:10,settle_s:0.,..Default::default()})?;
    wait(&mut app,"T4 eerste dosis",30.,|a|a.dev.running_test().is_some_and(|t|t.done>=1))?;std::thread::sleep(Duration::from_millis(300));
    app.stop();let t=wait(&mut app,"T4 STOP",2.,|a|a.dev.running_test().is_none())?;
    let r=app.db.test_run(run)?;let n=app.db.test_doses(run)?.len();
    if t>Duration::from_secs(1) || r.status!=testdb::RUN_STOPPED || n==0{return Err(format!("T4: {t:?} {} {n}",r.status));}
    out.push(format!("T4: STOP tijdens test na {} ms, status gestopt, {n} dosis/doses bewaard",t.as_millis()));
    // R1: recipe on one pot in simulation with the PLC emulator: parallel step {B,D} on LH1 in one CAN block, then E on LH2.
    app.dev.world.as_ref().ok_or("geen simulatie")?.lock().unwrap().max_travel_s=0.05;
    app.dev.calibrate(vec!["P_A01".into()],set.clone())?;wait(&mut app,"kalibratie P_A01",60.,|a|!a.dev.busy(DevId::Can))?;
    for (pump,ing) in [("P_C02","Smoke-B"),("P_C04","Smoke-D"),("P_A01","Smoke-E")]{let mut r=app.pumps.iter().find(|p|p.name==pump).cloned().ok_or("pomp")?;r.ingredient=ing.into();app.save_pump(r);}
    let item=|n:&str,g:f64|recipe::ItemDraft{ingredient:n.into(),grams:g,..Default::default()};
    let draft=recipe::RecipeDraft{name:"Smoke-recept".into(),tolerance_pct:2.,steps:vec![recipe::StepDraft{topup:false,items:vec![item("Smoke-B",2.),item("Smoke-D",1.)]},recipe::StepDraft{topup:false,items:vec![item("Smoke-E",1.5)]}],..Default::default()};
    let saved=app.db.save_recipe(ProfileKind::Vsd,None,&draft)?;
    let t0=Instant::now();let pot=app.dev.start_pot(&mut app.db,saved.version_id,false,0.)?;
    wait(&mut app,"R1 wacht op LH2",30.,|a|a.dev.pot.lock().unwrap().waiting.is_some())?;app.dev.pot_resume();
    wait(&mut app,"R1 pot klaar",30.,|a|!a.dev.pot_running())?;
    let r=app.db.pot_run(pot)?;let it=app.db.pot_items(pot)?;
    if r.status!=recipe::POT_DONE || it.len()!=3 || it[0].started_ms!=it[1].started_ms || !it[0].attributed || it[2].head!="LH2" || it.iter().any(|x|x.status!="ok"){
        return Err(format!("R1: {} {} {:?}",r.status,r.note,it.iter().map(|x|(&x.ingredient,&x.head,&x.status,x.started_ms)).collect::<Vec<_>>()));}
    out.push(format!("R1: pot {} (simulatie + PLC-emulator) klaar in {} ms; B en D in één CAN-blok (start {} ms), E op LH2; {}",pot,t0.elapsed().as_millis(),it[0].started_ms.unwrap_or(0)%100000,
        it.iter().map(|x|format!("{} {}→{} g",x.ingredient,pumptest::nl(x.requested_g,1),x.dosed_g.map(|g|pumptest::nl(g,3)).unwrap_or("—".into()))).collect::<Vec<_>>().join(", ")));
    // Recepten and Ingrediënten pages with this recipe open and its pot shown.
    {let ctx=eframe::egui::Context::default();style(&ctx);app.open_recipe(saved.recipe_id,saved.version_id);
        for page in [crate::ui::Page::Recepten,crate::ui::Page::Ingredienten]{app.page=page;for _ in 0..2{let out=ctx.run(Default::default(),|ctx|app.frame(ctx));if out.shapes.len()<10{return Err(format!("{} leeg",page.label()));}}}}
    // R5: STOP during the first (long) dose of a pot: stopped within 1 s, lift down in the emulator, run stopped.
    app.dev.world.as_ref().ok_or("geen simulatie")?.lock().unwrap().max_travel_s=30.;
    let pot=app.dev.start_pot(&mut app.db,saved.version_id,false,0.)?;
    wait(&mut app,"R5 doseren",10.,|a|a.dev.pot.lock().unwrap().lines.iter().any(|l|l.contains("doseren stap 1")))?;std::thread::sleep(Duration::from_millis(300));
    app.stop();let t=wait(&mut app,"R5 STOP",2.,|a|!a.dev.pot_running())?;
    let r=app.db.pot_run(pot)?;let msgs=app.dev.status(DevId::Plc).messages.join(" | ");
    let down=msgs.rfind("-> proces 1350 opdracht 8").map(|i|msgs[i..].contains("<- proces 1350 antwoord 9")).unwrap_or(false);
    if t>Duration::from_secs(1) || r.status!=recipe::POT_STOPPED || !down{return Err(format!("R5: {t:?} {} lift omlaag {down}",r.status));}
    wait(&mut app,"CAN vrij na R5",5.,|a|!a.dev.busy(DevId::Can))?;
    out.push(format!("R5: STOP tijdens pot na {} ms: pot gestopt, lift LH1 omlaag bevestigd door PLC-emulator",t.as_millis()));
    // P1: production, 3 orders / 7 pots (LH1 only, LH2 only, mixed) with PLC emulator and simulated scanner.
    app.dev.world.as_ref().ok_or("geen simulatie")?.lock().unwrap().max_travel_s=0.3;
    let one=|n:&str,i:&str|recipe::RecipeDraft{name:n.into(),tolerance_pct:2.,steps:vec![recipe::StepDraft{topup:false,items:vec![item(i,1.)]}],..Default::default()};
    let v1=app.db.save_recipe(ProfileKind::Vsd,None,&one("Smoke-LH1","Smoke-B"))?.version_id;let v2=app.db.save_recipe(ProfileKind::Vsd,None,&one("Smoke-LH2","Smoke-E"))?.version_id;
    app.db.create_order(ProfileKind::Vsd,v1,"Smoke LH1","",0,2)?;app.db.create_order(ProfileKind::Vsd,v2,"Smoke LH2","",0,2)?;app.db.create_order(ProfileKind::Vsd,saved.version_id,"Smoke gemengd","",0,3)?;
    app.dev.plc_trace.lock().unwrap().clear();
    let t=Instant::now();app.dev.start_production(&app.db,false)?;
    wait(&mut app,"P1 productie",120.,|a|!a.dev.production_running())?;let took=t.elapsed();
    let hist=app.db.history(ProfileKind::Vsd,&pump_studio::production::HistFilter::default(),50)?;
    let tr=app.dev.plc_trace.lock().unwrap().clone();
    let mut up=[false;2];let mut bad=0;let mut transports=0;
    for x in &tr{match (x.1,x.2){(1350,1)=>up[0]=x.4.is_ok(),(1350,8)=>up[0]=false,(2350,1)=>up[1]=x.4.is_ok(),(2350,8)=>up[1]=false,(1220,1)=>{transports+=1;if up!=[false,false]{bad+=1;}}_=>{}}}
    let first=tr.iter().find(|x|!matches!(x.1,1350|2350)).map(|x|(x.1,x.2));let last=tr.last().map(|x|(x.1,x.2));
    if hist.len()!=7 || hist.iter().any(|p|p.status!="klaar") || bad>0 || first!=Some((1310,1)) || last!=Some((1310,8)){
        return Err(format!("P1: {} potten {:?}, {bad} transport(en) met lift omhoog, eerste {first:?}, laatste {last:?}",hist.len(),hist.iter().map(|p|&p.status).collect::<Vec<_>>()));}
    // M1: measured overlap of the dosing (CAN blocks) of LH1 and LH2 on different pots.
    let blocks:Vec<(i64,String,i64,i64)>=hist.iter().flat_map(|p|app.db.pot_items(p.id).unwrap_or_default().into_iter().filter_map(|x|Some((x.pot_run_id,x.head,x.started_ms?,x.ended_ms?)))).collect();
    let ov=blocks.iter().filter(|x|x.1=="LH1").flat_map(|x|blocks.iter().filter(|y|y.1=="LH2" && y.0!=x.0).map(move|y|x.3.min(y.3)-x.2.max(y.2))).max().unwrap_or(0);
    let total:i64=blocks.iter().filter(|x|x.1=="LH1").flat_map(|x|blocks.iter().filter(|y|y.1=="LH2" && y.0!=x.0).map(move|y|(x.3.min(y.3)-x.2.max(y.2)).max(0))).sum();
    if ov<100{return Err(format!("P1: doseren LH1/LH2 overlapt maar {ov} ms"));}
    out.push(format!("P1: productie 3 orders / 7 potten (simulatie + PLC-emulator + scanner) klaar in {} ms; {transports} transporten, alle met beide liften omlaag; eerst liften omlaag (9), dan 1310 Start, Stop laatst; doseren LH1 en LH2 op verschillende potten overlapt gemeten max {ov} ms per blokpaar, {total} ms totaal",took.as_millis()));
    // P3: Afbreken (STOP) during production: pumps stop, lifts down, Abort to line/units, pots kept as afgebroken.
    app.dev.world.as_ref().ok_or("geen simulatie")?.lock().unwrap().max_travel_s=30.;
    app.db.create_order(ProfileKind::Vsd,saved.version_id,"Smoke afbreken","",0,2)?;
    app.dev.start_production(&app.db,false)?;
    wait(&mut app,"P3 lift omhoog",30.,|a|a.dev.lift_up(Head::LH1))?;std::thread::sleep(Duration::from_millis(400));
    app.stop();let t=wait(&mut app,"P3 afbreken",5.,|a|!a.dev.production_running())?;
    let line=app.db.line_state()?.ok_or("geen lijnbeeld")?;
    let aborted=line.slots.iter().filter(|s|matches!(s,pump_studio::production::Slot::Pot(_))).count();
    if t>Duration::from_millis(1500) || line.state!="afgebroken" || app.dev.lift_up(Head::LH1) || aborted==0{return Err(format!("P3: {t:?}, {}, {aborted} pot(ten)",line.state));}
    wait(&mut app,"CAN vrij na P3",5.,|a|!a.dev.busy(DevId::Can))?;
    out.push(format!("P3: STOP tijdens productie na {} ms afgebroken: liften omlaag, Abort naar lijn/units, {aborted} pot(ten) als afgebroken op de lijn bewaard",t.as_millis()));
    {let ctx=eframe::egui::Context::default();style(&ctx);let mut slow=Duration::ZERO;
        for page in [crate::ui::Page::Productie,crate::ui::Page::Machine,crate::ui::Page::Monitors]{app.page=page;for _ in 0..3{let f=Instant::now();let o=ctx.run(Default::default(),|ctx|app.frame(ctx));slow=slow.max(f.elapsed());if o.shapes.len()<10{return Err(format!("{} leeg",page.label()));}}}
        out.push(format!("Productie, Machine/lijn en Monitors getekend met orders, historie en afgebroken lijn; langste frame {} ms",slow.as_millis()));
        // Analyse page: background calculation, frames keep drawing; then export of the selection (with simulation).
        app.page=crate::ui::Page::Analyse;app.anaui.include_sim=true;let t=Instant::now();let mut slow=Duration::ZERO;let mut frames=0;
        loop{let f=Instant::now();let o=ctx.run(Default::default(),|ctx|{app.tick();app.frame(ctx);});slow=slow.max(f.elapsed());frames+=1;
            if o.shapes.len()<10{return Err("Analyse leeg".into());}
            if app.anaui_ready(){break;}if t.elapsed()>Duration::from_secs(20){return Err("Analyse niet berekend".into());}std::thread::sleep(Duration::from_millis(10));}
        out.push(format!("Analyse (met simulatie) berekend en getekend in {} ms over {frames} frame(s), langste frame {} ms",t.elapsed().as_millis(),slow.as_millis()));}
    {
        let f=pump_studio::analysis::Filter{include_sim:true,..Default::default()};let t=Instant::now();let a=app.db.analysis(&f)?;let q=t.elapsed();
        if a.pots.len()<9 || a.items.is_empty() || a.ingredients.is_empty() || a.cals.is_empty() || a.tests.is_empty(){return Err(format!("Analyse: {} potten, {} items, {} kalibraties, {} testdoses",a.pots.len(),a.items.len(),a.cals.len(),a.tests.len()));}
        let dir=root.join("results").join("analyse");let t=Instant::now();let x=pump_studio::analysis::export(&a,&dir,true)?;let c=pump_studio::analysis::export(&a,&dir,false)?;let et=t.elapsed();
        let b=std::fs::read(&x[0]).map_err(|e|e.to_string())?;
        if !b.starts_with(b"PK\x03\x04") || !String::from_utf8_lossy(&b).contains("xl/worksheets/sheet4.xml") || c.len()!=4{return Err("Export onvolledig".into());}
        let csv=std::fs::read_to_string(&c[0]).map_err(|e|e.to_string())?;if !csv.starts_with('\u{feff}') || !csv.contains(';'){return Err("CSV niet NL".into());}
        out.push(format!("Analyse: {} potten, {} ingrediëntregels, {} testdoses, {} kalibraties in {} ms; export xlsx ({} kB, 4 tabbladen) + 4 CSV in {} ms",
            a.pots.len(),a.items.len(),a.tests.len(),a.cals.len(),q.as_millis(),b.len()/1024,et.as_millis()));
    }
    let n=app.db.clear_line()?;out.push(format!("lijn leeggemaakt ({n} pot(ten))"));
    // Pump edit stored immediately.
    let mut row=app.pumps.iter().find(|p|p.name=="P_B07").cloned().unwrap();row.ingredient="Smoke-ingrediënt".into();row.enabled=false;row.head=Some(Head::LH1);app.save_pump(row.clone());
    let check=Db::open(app.db.path())?;let stored=check.pumps(row.profile_id)?.into_iter().find(|p|p.id==row.id).unwrap();
    if stored.ingredient!="Smoke-ingrediënt" || stored.enabled || stored.head!=Some(Head::LH1){return Err("Pompwijziging niet opgeslagen".into());}
    out.push("pompwijziging direct in database".into());
    // K8: profile switch with VSD devices connected and a dose running; frames keep drawing.
    app.dev.pumps(vec!["P_D11".into()],PumpOp::Dispense,40*51200)?;std::thread::sleep(Duration::from_millis(100));
    let ctx=eframe::egui::Context::default();style(&ctx);
    let (max,total,n)=switch_frames(&mut app,&ctx)?;
    if max>Duration::from_millis(100){return Err(format!("K8: frame van {max:?} tijdens profielwissel"));}
    out.push(format!("K8: profielwissel in {} ms over {n} frame(s), langste frame {} ms",total.as_millis(),max.as_millis()));
    wait(&mut app,"VSD verbroken",5.,|a|DevId::VSD.iter().all(|d|!a.dev.is_on(*d)))?;
    if app.pumps.len()!=1{return Err("Keramisch profiel: verwacht 1 pomp".into());}
    app.dev.settings.bench.settle_s=0.;
    app.dev.connect(DevId::Bench,true)?;wait(&mut app,"pompbank",5.,|a|on(a,DevId::Bench) && !a.dev.busy(DevId::Bench))?;
    app.dev.connect(DevId::BenchScale,true)?;wait(&mut app,"weegschaal pompbank",5.,|a|on(a,DevId::BenchScale) && !a.dev.busy(DevId::Bench))?;
    wait(&mut app,"live gewicht pompbank",5.,|a|a.dev.bench.live_g.is_some())?;
    app.dev.bench_action(Action::Zero)?;wait(&mut app,"zero",5.,|a|!a.dev.busy(DevId::Bench))?;
    let target=jog_target(app.dev.bench.position,app.dev.bench.config.steps_rev,0,0,1,1.)?;
    app.dev.bench_action(Action::Jog{target,weigh:true})?;wait(&mut app,"vooruit",5.,|a|!a.dev.busy(DevId::Bench))?;
    let d=app.dev.bench.manual.last().and_then(|m|m.delta_g).unwrap_or_default();
    if app.dev.bench.position!=target || (d-1.345).abs()>1e-6{return Err(format!("Keramisch vooruit wijkt af: {} st, {d} g",app.dev.bench.position));}
    wait(&mut app,"positie pompbank in database",3.,|a|a.pumps[0].position==Some(target as f64))?;
    out.push(format!("keramisch: verbonden met weegschaal, 1 omw vooruit gewogen {d:.4} g"));
    // K5: ceramic calibration and 360° curve, stored as active.
    app.dev.bench_action(Action::Calibrate{revolutions:1,repeats:3,prime:0})?;wait(&mut app,"keramische kalibratie",10.,|a|!a.dev.busy(DevId::Bench) && a.dev.bench_cal_version()>=1)?;
    app.dev.bench_action(Action::Characterize{interval:Interval::Degrees(30.),repeats:2})?;wait(&mut app,"meetcurve",20.,|a|!a.dev.busy(DevId::Bench) && a.dev.bench_cal_version()>=2)?;
    let pump=app.pumps[0].name.clone();
    let kc=app.db.active_calibration(ProfileKind::Keramisch,&pump,calib::KIND_BENCH_CAL)?.ok_or("K5: kalibratie niet opgeslagen")?;
    let kk=app.db.active_calibration(ProfileKind::Keramisch,&pump,calib::KIND_BENCH_CURVE)?.ok_or("K5: meetcurve niet opgeslagen")?;
    if (kc.g_per_rev.unwrap_or(0.)-1.345).abs()>1e-6 || (kk.g_per_rev.unwrap_or(0.)-1.345).abs()>1e-6 || app.db.bench_curve(kk.id)?.is_none(){return Err("K5: waarden wijken af".into());}
    out.push(format!("K5: keramische kalibratie {:.4} g/omw en meetcurve {:.4} g/omw opgeslagen (actief)",kc.g_per_rev.unwrap_or(0.),kk.g_per_rev.unwrap_or(0.)));
    app.dev.shutdown();wait(&mut app,"afsluiten",5.,|a|!a.dev.bench.connected)?;
    // Log index in SQLite.
    std::thread::sleep(Duration::from_millis(300));
    let n=Db::open(app.db.path())?.log_count("")?;if n<20{return Err(format!("Logindex bevat {n} regels"));}
    out.push(format!("logindex: {n} regels in database"));
    applog::set_sink(None);
    Ok(out)
}
