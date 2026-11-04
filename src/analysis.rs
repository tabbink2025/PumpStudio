//! Analyse: history per pump (calibration g/stroke trend, test deviation per weight over time), production deviation
//! per ingredient and per pump, pots per recipe with % within tolerance, a dashboard (today / 7 days), and export of
//! the filtered selection to CSV (NL) and .xlsx. Computed on a background thread from its own connection.
//!
//! Definitions (documented in LEESMIJ):
//! - Simulation is left out unless `include_sim` is set (then the UI marks it clearly; exports have a column "Soort").
//! - Test doses: only measured, not excluded doses count (an excluded dose never counts).
//! - Production items: measured items with a judgement (ok / te weinig / te veel). Parallel (attributed) items count
//!   like any other item; their number is shown per ingredient ("toegerekend").
//! - A pot is "binnen tolerantie" when it is klaar and all its measured items are ok. Head filter: only that head's items.
use crate::{db::{Db,ProfileKind},model::Result,xlsx::{Cell,Sheet}};
use rusqlite::params;
use std::collections::BTreeMap;

fn e(x:rusqlite::Error)->String{format!("Database: {x}")}
pub fn now_ms()->i64{std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d|d.as_millis() as i64).unwrap_or(0)}
/// Start of the local (CET/CEST) day of `ms`: the moment the local clock showed 00:00 on that date. On a DST change
/// day the offset at midnight differs from the offset now, so the candidate is corrected against the local date.
pub fn day_start(ms:i64)->i64{
    let st=|m:i64|crate::applog::local_stamp(m);
    let tod=|t:&str|{let p=|a:usize,b:usize|t.get(a..b).and_then(|x|x.parse::<i64>().ok()).unwrap_or(0);(p(11,13)*3600+p(14,16)*60+p(17,19))*1000+p(20,23)};
    let date=st(ms)[..10].to_string();
    let mut c=ms-tod(&st(ms));
    for _ in 0..3{
        let t=st(c);let d=tod(&t);
        if d==0 && t[..10]==date{break;}
        // Same date: go back by the remaining time of day; previous date (e.g. 23:00): forward to its midnight.
        c=if t[..10]==date{c-d}else{c+86_400_000-d};
    }
    c
}

#[derive(Clone,Debug,Default,PartialEq)]
pub struct Filter{pub profile:Option<ProfileKind>,pub from_ms:Option<i64>,pub to_ms:Option<i64>,pub head:Option<String>,pub include_sim:bool}
#[derive(Clone,Debug,Default,PartialEq)]
pub struct ItemA{pub pot:i64,pub barcode:String,pub recipe:String,pub version:i64,pub order:String,pub started_ms:i64,pub pot_status:String,pub simulation:bool,
    pub ingredient:String,pub pump:String,pub head:String,pub requested_g:f64,pub dosed_g:Option<f64>,pub deviation_pct:Option<f64>,pub tolerance_pct:f64,
    pub status:String,pub attributed:bool,pub topped_up:bool}
#[derive(Clone,Debug,Default,PartialEq)]
pub struct PotA{pub id:i64,pub barcode:String,pub recipe:String,pub version:i64,pub order:String,pub mode:String,pub started_ms:i64,pub status:String,pub simulation:bool,
    pub items:usize,pub items_ok:usize,pub within:bool}
#[derive(Clone,Debug,Default,PartialEq)]
pub struct DevStat{pub name:String,pub n:usize,pub mean_pct:f64,pub mean_abs_pct:f64,pub sd_pct:f64,pub within:usize,pub within_pct:f64,pub attributed:usize}
#[derive(Clone,Debug,Default,PartialEq)]
pub struct RecipeStat{pub recipe:String,pub pots:usize,pub within:usize,pub within_pct:f64}
#[derive(Clone,Debug,Default,PartialEq)]
pub struct TestA{pub pump:String,pub run:i64,pub time_ms:i64,pub requested_g:f64,pub dosed_g:f64,pub err_pct:f64,pub simulation:bool}
#[derive(Clone,Debug,Default,PartialEq)]
pub struct TestStat{pub pump:String,pub requested_g:f64,pub n:usize,pub mean_pct:f64,pub sd_pct:f64,pub first_ms:i64,pub last_ms:i64}
#[derive(Clone,Debug,Default,PartialEq)]
pub struct CalA{pub pump:String,pub date_ms:i64,pub kind:String,pub g_per_stroke:Option<f64>,pub g_per_rev:Option<f64>,pub simulation:bool,pub status:String,pub source:String}
#[derive(Clone,Debug,Default,PartialEq)]
pub struct Dashboard{pub today:usize,pub today_within_pct:Option<f64>,pub week:usize,pub week_within_pct:Option<f64>,pub worst_ingredients:Vec<DevStat>,pub worst_pumps:Vec<DevStat>}
#[derive(Clone,Debug,Default)]
pub struct Analysis{pub filter:Filter,pub pots:Vec<PotA>,pub items:Vec<ItemA>,pub ingredients:Vec<DevStat>,pub pumps:Vec<DevStat>,pub recipes:Vec<RecipeStat>,
    pub tests:Vec<TestA>,pub test_stats:Vec<TestStat>,pub cals:Vec<CalA>,pub dash:Dashboard,pub took_ms:u128}

fn stats(name:&str,v:&[(f64,bool,bool)])->DevStat{
    let n=v.len();if n==0{return DevStat{name:name.into(),..Default::default()};}
    let mean=v.iter().map(|x|x.0).sum::<f64>()/n as f64;
    let sd=if n>1{(v.iter().map(|x|(x.0-mean).powi(2)).sum::<f64>()/(n-1) as f64).sqrt()}else{0.};
    let within=v.iter().filter(|x|x.1).count();
    DevStat{name:name.into(),n,mean_pct:mean,mean_abs_pct:v.iter().map(|x|x.0.abs()).sum::<f64>()/n as f64,sd_pct:sd,within,within_pct:100.*within as f64/n as f64,attributed:v.iter().filter(|x|x.2).count()}
}
fn judged(s:&str)->bool{matches!(s,"ok"|"te_weinig"|"te_veel")}

impl Db{
    /// Production items (with their pot) of the selection.
    fn analysis_items(&self,pid:i64,f:&Filter)->Result<(Vec<PotA>,Vec<ItemA>)>{
        let mut st=self.conn.prepare("SELECT p.id,coalesce(p.barcode,''),r.name,v.version,coalesce(o.name,''),p.started_ms,p.status,p.simulation,p.mode,
              i.ingredient,i.pump,i.head,i.requested_g,i.dosed_g,i.deviation_pct,i.tolerance_pct,i.status,i.attributed,i.topped_up
            FROM pot_run p JOIN recipe_version v ON v.id=p.recipe_version_id JOIN recipe r ON r.id=v.recipe_id LEFT JOIN prod_order o ON o.id=p.order_id
            LEFT JOIN pot_item i ON i.pot_run_id=p.id
            WHERE p.profile_id=?1 AND p.status NOT IN ('wacht','geannuleerd','loopt') AND (?2 IS NULL OR p.started_ms>=?2) AND (?3 IS NULL OR p.started_ms<?3) AND (?4=1 OR p.simulation=0)
            ORDER BY p.started_ms,p.id,i.step_idx,i.item_idx").map_err(e)?;
        let mut pots:Vec<PotA>=vec![];let mut items=vec![];
        let rows=st.query_map(params![pid,f.from_ms,f.to_ms,f.include_sim as i64],|r|{
            let pot=PotA{id:r.get(0)?,barcode:r.get(1)?,recipe:r.get(2)?,version:r.get(3)?,order:r.get(4)?,started_ms:r.get(5)?,status:r.get(6)?,simulation:r.get::<_,i64>(7)?!=0,mode:r.get(8)?,..Default::default()};
            let ing:Option<String>=r.get(9)?;
            let item=match ing{None=>None,Some(ingredient)=>Some(ItemA{pot:pot.id,barcode:pot.barcode.clone(),recipe:pot.recipe.clone(),version:pot.version,order:pot.order.clone(),started_ms:pot.started_ms,
                pot_status:pot.status.clone(),simulation:pot.simulation,ingredient,pump:r.get(10)?,head:r.get(11)?,requested_g:r.get(12)?,dosed_g:r.get(13)?,deviation_pct:r.get(14)?,
                tolerance_pct:r.get(15)?,status:r.get(16)?,attributed:r.get::<_,i64>(17)?!=0,topped_up:r.get::<_,i64>(18)?!=0})};
            Ok((pot,item))
        }).map_err(e)?;
        for x in rows{
            let (pot,item)=x.map_err(e)?;
            if pots.last().map(|p|p.id)!=Some(pot.id){pots.push(pot);}
            if let Some(i)=item{if f.head.as_ref().is_none_or(|h|*h==i.head){
                let p=pots.last_mut().unwrap();if judged(&i.status) && i.dosed_g.is_some(){p.items+=1;if i.status=="ok"{p.items_ok+=1;}}
                items.push(i);}}
        }
        // Head filter: only pots with items on that head.
        if f.head.is_some(){let with:std::collections::HashSet<i64>=items.iter().map(|i|i.pot).collect();pots.retain(|p|with.contains(&p.id));}
        for p in pots.iter_mut(){p.within=p.status=="klaar" && p.items>0 && p.items==p.items_ok;}
        Ok((pots,items))
    }
    pub fn analysis(&self,f:&Filter)->Result<Analysis>{
        let t=std::time::Instant::now();
        let profile=f.profile.unwrap_or(ProfileKind::Vsd);let pid=self.profile(profile)?.id;
        let (pots,items)=self.analysis_items(pid,f)?;
        let group=|key:&dyn Fn(&ItemA)->String|{
            let mut m:BTreeMap<String,Vec<(f64,bool,bool)>>=BTreeMap::new();
            for i in items.iter().filter(|i|judged(&i.status) && i.dosed_g.is_some()){if let Some(d)=i.deviation_pct{m.entry(key(i)).or_default().push((d,i.status=="ok",i.attributed));}}
            m.into_iter().map(|(k,v)|stats(&k,&v)).collect::<Vec<_>>()
        };
        let ingredients=group(&|i|i.ingredient.clone());let pumps=group(&|i|i.pump.clone());
        let mut rm:BTreeMap<String,(usize,usize)>=BTreeMap::new();
        for p in &pots{let e=rm.entry(format!("{} v{}",p.recipe,p.version)).or_default();e.0+=1;if p.within{e.1+=1;}}
        let recipes=rm.into_iter().map(|(k,(n,w))|RecipeStat{recipe:k,pots:n,within:w,within_pct:if n>0{100.*w as f64/n as f64}else{0.}}).collect();
        // Test doses (never excluded ones), with the pump's head for the head filter.
        let mut st=self.conn.prepare("SELECT t.pump_name,t.id,d.time_ms,d.requested_g,d.dosed_g,t.simulation,coalesce(pm.head,'')
            FROM test_dose d JOIN test_run t ON t.id=d.run_id LEFT JOIN pump pm ON pm.profile_id=t.profile_id AND pm.name=t.pump_name
            WHERE t.profile_id=?1 AND d.excluded=0 AND d.dosed_g IS NOT NULL AND (?2 IS NULL OR d.time_ms>=?2) AND (?3 IS NULL OR d.time_ms<?3) AND (?4=1 OR t.simulation=0)
            ORDER BY d.time_ms,d.id").map_err(e)?;
        let mut tests=vec![];
        for x in st.query_map(params![pid,f.from_ms,f.to_ms,f.include_sim as i64],|r|Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,r.get::<_,i64>(2)?,r.get::<_,f64>(3)?,r.get::<_,f64>(4)?,r.get::<_,i64>(5)?!=0,r.get::<_,String>(6)?))).map_err(e)?{
            let (pump,run,time_ms,req,dosed,sim,head)=x.map_err(e)?;
            if f.head.as_ref().is_some_and(|h|*h!=head){continue;}
            tests.push(TestA{pump,run,time_ms,requested_g:req,dosed_g:dosed,err_pct:100.*(dosed-req)/req,simulation:sim});
        }
        let mut tm:BTreeMap<(String,i64),Vec<&TestA>>=BTreeMap::new();
        for t in &tests{tm.entry((t.pump.clone(),(t.requested_g*1000.).round() as i64)).or_default().push(t);}
        let test_stats=tm.into_iter().map(|((p,_),v)|{let s=stats(&p,&v.iter().map(|t|(t.err_pct,false,false)).collect::<Vec<_>>());
            TestStat{pump:p,requested_g:v[0].requested_g,n:s.n,mean_pct:s.mean_pct,sd_pct:s.sd_pct,first_ms:v[0].time_ms,last_ms:v[v.len()-1].time_ms}}).collect();
        // Calibrations (also imported and archived ones: the trend).
        let mut st=self.conn.prepare("SELECT c.pump_name,c.date_ms,c.kind,c.g_per_stroke,c.g_per_rev,c.simulation,c.status,c.source,coalesce(pm.head,'')
            FROM calibration c LEFT JOIN pump pm ON pm.profile_id=c.profile_id AND pm.name=c.pump_name
            WHERE c.profile_id=?1 AND (?2 IS NULL OR c.date_ms>=?2) AND (?3 IS NULL OR c.date_ms<?3) AND (?4=1 OR c.simulation=0) ORDER BY c.pump_name,c.date_ms").map_err(e)?;
        let mut cals=vec![];
        for x in st.query_map(params![pid,f.from_ms,f.to_ms,f.include_sim as i64],|r|Ok((CalA{pump:r.get(0)?,date_ms:r.get(1)?,kind:r.get(2)?,g_per_stroke:r.get(3)?,g_per_rev:r.get(4)?,simulation:r.get::<_,i64>(5)?!=0,status:r.get(6)?,source:r.get(7)?},r.get::<_,String>(8)?))).map_err(e)?{
            let (c,head)=x.map_err(e)?;if f.head.as_ref().is_some_and(|h|*h!=head){continue;}cals.push(c);
        }
        let dash=self.dashboard(pid,f,now_ms())?;
        Ok(Analysis{filter:f.clone(),pots,items,ingredients,pumps,recipes,tests,test_stats,cals,dash,took_ms:t.elapsed().as_millis()})
    }
    /// Dashboard: pots today / last 7 days (klaar) and % within tolerance; worst 5 ingredients and pumps of the 7 days
    /// (mean absolute deviation). Uses the head and simulation filter, not the period.
    pub fn dashboard(&self,pid:i64,f:&Filter,now:i64)->Result<Dashboard>{
        let week=Filter{from_ms:Some(now-7*86_400_000),to_ms:None,..f.clone()};
        let (pots,items)=self.analysis_items(pid,&week)?;
        let today=day_start(now);
        let done=|from:i64|{let v:Vec<&PotA>=pots.iter().filter(|p|p.status=="klaar" && p.started_ms>=from).collect();
            let w=v.iter().filter(|p|p.within).count();(v.len(),if v.is_empty(){None}else{Some(100.*w as f64/v.len() as f64)})};
        let (t,tp)=done(today);let (wk,wp)=done(now-7*86_400_000);
        let worst=|key:&dyn Fn(&ItemA)->String|{
            let mut m:BTreeMap<String,Vec<(f64,bool,bool)>>=BTreeMap::new();
            for i in items.iter().filter(|i|judged(&i.status) && i.dosed_g.is_some()){if let Some(d)=i.deviation_pct{m.entry(key(i)).or_default().push((d,i.status=="ok",i.attributed));}}
            let mut v:Vec<DevStat>=m.into_iter().map(|(k,v)|stats(&k,&v)).collect();
            v.sort_by(|a,b|b.mean_abs_pct.total_cmp(&a.mean_abs_pct).then(a.name.cmp(&b.name)));v.truncate(5);v
        };
        Ok(Dashboard{today:t,today_within_pct:tp,week:wk,week_within_pct:wp,worst_ingredients:worst(&|i|i.ingredient.clone()),worst_pumps:worst(&|i|i.pump.clone())})
    }
    /// Log retention: delete log rows older than `days` (0 = keep all). Returns the number deleted.
    pub fn prune_log(&self,days:i64,now:i64)->Result<usize>{
        if days<=0{return Ok(0);}
        let cutoff=crate::applog::local_stamp(now-days*86_400_000);
        let mut n=0;
        // In batches, so the log writer is never held up for long.
        loop{let k=self.conn.execute("DELETE FROM log WHERE id IN (SELECT id FROM log WHERE time<?1 LIMIT 5000)",[&cutoff]).map_err(e)?;n+=k;if k<5000{break;}}
        Ok(n)
    }
}

// ---------- export ----------
fn nl(v:f64,d:usize)->String{crate::pumptest::nl(v,d)}
fn q(s:&str)->String{if s.contains([';','"','\n']){format!("\"{}\"",s.replace('"',"\"\""))}else{s.to_string()}}
fn soort(sim:bool)->&'static str{if sim{"simulatie"}else{"hardware"}}
fn ot(v:Option<f64>)->Cell{v.map(Cell::Num).unwrap_or(Cell::Empty)}
/// The four tables of the selection: Potten, Ingrediënten (results per item), Testen, Kalibraties.
pub fn sheets(a:&Analysis)->Vec<Sheet>{
    let s=|v:&[&str]|v.iter().map(|x|x.to_string()).collect::<Vec<_>>();
    vec![
        Sheet{name:"Potten".into(),header:s(&["Pot","Code","Datum","Order","Recept","Versie","Soort","Modus","Status","Items gemeten","Items binnen tolerantie","Binnen tolerantie"]),
            rows:a.pots.iter().map(|p|vec![Cell::Int(p.id),Cell::Text(p.barcode.clone()),Cell::Date(p.started_ms),Cell::Text(p.order.clone()),Cell::Text(p.recipe.clone()),Cell::Int(p.version),
                Cell::Text(soort(p.simulation).into()),Cell::Text(p.mode.clone()),Cell::Text(p.status.clone()),Cell::Int(p.items as i64),Cell::Int(p.items_ok as i64),Cell::Text(if p.within{"ja"}else{"nee"}.into())]).collect()},
        Sheet{name:"Ingrediënten".into(),header:s(&["Pot","Code","Datum","Recept","Versie","Soort","Ingrediënt","Pomp","Kop","Gevraagd g","Gedoseerd g","Afwijking %","Tolerantie %","Oordeel","Toegerekend","Bijgedoseerd"]),
            rows:a.items.iter().map(|i|vec![Cell::Int(i.pot),Cell::Text(i.barcode.clone()),Cell::Date(i.started_ms),Cell::Text(i.recipe.clone()),Cell::Int(i.version),Cell::Text(soort(i.simulation).into()),
                Cell::Text(i.ingredient.clone()),Cell::Text(i.pump.clone()),Cell::Text(i.head.clone()),Cell::Num(i.requested_g),ot(i.dosed_g),ot(i.deviation_pct),Cell::Num(i.tolerance_pct),
                Cell::Text(i.status.clone()),Cell::Text(if i.attributed{"ja"}else{"nee"}.into()),Cell::Text(if i.topped_up{"ja"}else{"nee"}.into())]).collect()},
        Sheet{name:"Testen".into(),header:s(&["Pomp","Test","Datum","Soort","Gevraagd g","Gedoseerd g","Afwijking %"]),
            rows:a.tests.iter().map(|t|vec![Cell::Text(t.pump.clone()),Cell::Int(t.run),Cell::Date(t.time_ms),Cell::Text(soort(t.simulation).into()),Cell::Num(t.requested_g),Cell::Num(t.dosed_g),Cell::Num(t.err_pct)]).collect()},
        Sheet{name:"Kalibraties".into(),header:s(&["Pomp","Datum","Soort kalibratie","g/slag","g/omw","Soort","Status","Bron"]),
            rows:a.cals.iter().map(|c|vec![Cell::Text(c.pump.clone()),Cell::Date(c.date_ms),Cell::Text(crate::calib::kind_label(&c.kind).into()),ot(c.g_per_stroke),ot(c.g_per_rev),
                Cell::Text(soort(c.simulation).into()),Cell::Text(c.status.clone()),Cell::Text(c.source.clone())]).collect()},
    ]
}
/// CSV for Excel (NL): one file per table, semicolon, decimal comma, UTF-8 with BOM; dates as local "dd-mm-jjjj uu:mm".
pub fn csv_files(a:&Analysis)->Vec<(String,Vec<u8>)>{
    let date=|ms:i64|{let t=crate::applog::local_time(ms);format!("{}-{}-{} {}",&t[8..10],&t[5..7],&t[0..4],&t[11..16])};
    sheets(a).into_iter().map(|sh|{
        let mut out=String::from("\u{feff}");
        out+=&sh.header.iter().map(|h|q(h)).collect::<Vec<_>>().join(";");out+="\n";
        for r in &sh.rows{
            out+=&r.iter().map(|c|match c{Cell::Text(t)=>q(t),Cell::Num(f)=>nl(*f,4),Cell::Int(i)=>i.to_string(),Cell::Date(ms)=>date(*ms),Cell::Empty=>String::new()}).collect::<Vec<_>>().join(";");out+="\n";
        }
        (sh.name.clone(),out.into_bytes())
    }).collect()
}
/// Write the export (xlsx or the CSV files) into `dir`; returns the written paths.
pub fn export(a:&Analysis,dir:&std::path::Path,xlsx:bool)->Result<Vec<std::path::PathBuf>>{
    std::fs::create_dir_all(dir).map_err(|e|e.to_string())?;
    let stamp=crate::applog::local_stamp(now_ms()).replace([' ',':','.'],"-");let stamp=&stamp[..19];
    let sim=if a.filter.include_sim{"_met_simulatie"}else{""};
    if xlsx{
        let p=dir.join(format!("analyse_{stamp}{sim}.xlsx"));std::fs::write(&p,crate::xlsx::workbook(&sheets(a))?).map_err(|e|e.to_string())?;Ok(vec![p])
    }else{
        csv_files(a).into_iter().map(|(n,b)|{let p=dir.join(format!("analyse_{stamp}{sim}_{}.csv",n.to_lowercase().replace('ë',"e")));std::fs::write(&p,b).map_err(|e|e.to_string())?;Ok(p)}).collect()
    }
}

#[cfg(test)] mod tests{
    use super::*;
    use crate::recipe::tests::{item,step,r1_setup,temp,cleanup};
    /// Fixed data (hand-calculated in the asserts): recipe Alfa, pots 1–2 hardware, pot 3 simulation; tests on P_C01.
    fn fixture(name:&str,now:i64)->(std::path::PathBuf,Db){
        let dir=temp(name);let mut db=Db::open(&dir.join("p.db")).unwrap();r1_setup(&mut db);
        let v=db.save_recipe(ProfileKind::Vsd,None,&crate::recipe::RecipeDraft{name:"Alfa".into(),steps:vec![step(vec![item("A",1.)]),step(vec![item("B",2.)])],..Default::default()}).unwrap().version_id;
        let pid=db.profile(ProfileKind::Vsd).unwrap().id;
        let pot=|sim:bool,items:&[(&str,&str,&str,f64,f64,&str,bool)]|{
            db.conn.execute("INSERT INTO pot_run(profile_id,recipe_version_id,simulation,mode,status,started_ms) VALUES(?1,?2,?3,'productie','klaar',?4)",params![pid,v,sim as i64,now-1000]).unwrap();
            let id=db.conn.last_insert_rowid();
            for (k,(ing,pump,head,req,dosed,st,attr)) in items.iter().enumerate(){
                db.conn.execute("INSERT INTO pot_item(pot_run_id,step_idx,item_idx,ingredient,pump,head,requested_g,dosed_g,deviation_pct,tolerance_pct,status,attributed) VALUES(?1,?2,0,?3,?4,?5,?6,?7,?8,1,?9,?10)",
                    params![id,k as i64,ing,pump,head,req,dosed,100.*(dosed-req)/req,st,*attr as i64]).unwrap();
            }
        };
        pot(false,&[("A","P_C01","LH1",1.,1.005,"ok",false),("B","P_C02","LH1",2.,1.97,"te_weinig",false)]);
        pot(false,&[("A","P_C01","LH1",1.,0.99,"ok",false),("B","P_C02","LH1",2.,2.01,"ok",true)]);
        pot(true,&[("A","P_C01","LH1",1.,1.10,"te_veel",false)]);
        // Tests on P_C01: 1 g → +1 %, −1 %, +2 % (excluded); 5 g → +1 %; simulation run +5 %.
        for (sim,doses) in [(false,vec![(1.,1.01,false),(1.,0.99,false),(1.,1.02,true),(5.,5.05,false)]),(true,vec![(1.,1.05,false)])]{
            db.conn.execute("INSERT INTO test_run(profile_id,pump_name,simulation,mode,tolerance_pct,status,started_ms) VALUES(?1,'P_C01',?2,'reeks',1,'klaar',?3)",params![pid,sim as i64,now-5000]).unwrap();
            let run=db.conn.last_insert_rowid();
            for (k,(req,dosed,ex)) in doses.iter().enumerate(){db.conn.execute("INSERT INTO test_dose(run_id,nr,requested_g,dosed_g,status,excluded,time_ms) VALUES(?1,?2,?3,?4,'ok',?5,?6)",params![run,k as i64+1,req,dosed,*ex as i64,now-4000+k as i64]).unwrap();}
        }
        for (g,st) in [(0.50,"archief"),(0.52,"actief")]{db.conn.execute("INSERT INTO calibration(profile_id,pump_name,date_ms,kind,simulation,g_per_stroke,status) VALUES(?1,'P_C01',?2,'vsd',0,?3,?4)",params![pid,now-3000,g,st]).unwrap();}
        (dir,db)
    }
    #[test]fn n1_numbers_match_hand_calculation(){
        let now=now_ms();let (dir,db)=fixture("n1",now);
        let a=db.analysis(&Filter::default()).unwrap();
        // Simulation left out: pots 1–2.
        assert_eq!(a.pots.len(),2);assert_eq!(a.pots.iter().map(|p|p.within).collect::<Vec<_>>(),vec![false,true]);
        let ia=&a.ingredients[0];let ib=&a.ingredients[1];
        // A: +0,5 and −1,0 → mean −0,25, mean |.| 0,75, sd √((0,75²·2)/1) = 1,0607, 2/2 within.
        assert_eq!((ia.name.as_str(),ia.n,ia.within),("A",2,2));assert!((ia.mean_pct+0.25).abs()<1e-9 && (ia.mean_abs_pct-0.75).abs()<1e-9 && (ia.sd_pct-1.0606601717798212).abs()<1e-9,"{ia:?}");
        // B: −1,5 and +0,5 → mean −0,5, mean |.| 1,0, 1/2 within (50 %), 1 attributed.
        assert_eq!((ib.name.as_str(),ib.n,ib.within,ib.attributed),("B",2,1,1));assert!((ib.mean_pct+0.5).abs()<1e-9 && (ib.mean_abs_pct-1.).abs()<1e-9 && (ib.within_pct-50.).abs()<1e-9);
        assert_eq!(a.recipes,vec![RecipeStat{recipe:"Alfa v1".into(),pots:2,within:1,within_pct:50.}]);
        // Tests: excluded dose never counts. P_C01 1 g: n 2 mean 0 sd 1,4142; 5 g: n 1 mean +1.
        assert_eq!(a.tests.len(),3);let t1=&a.test_stats[0];let t5=&a.test_stats[1];
        assert_eq!((t1.requested_g,t1.n),(1.,2));assert!(t1.mean_pct.abs()<1e-9 && (t1.sd_pct-std::f64::consts::SQRT_2).abs()<1e-9,"{t1:?}");
        assert_eq!((t5.requested_g,t5.n),(5.,1));assert!((t5.mean_pct-1.).abs()<1e-9);
        assert_eq!(a.cals.iter().map(|c|c.g_per_stroke.unwrap()).collect::<Vec<_>>(),vec![0.50,0.52],"trend incl. archief");
        // Dashboard: today 2 pots, 1 within → 50 %; worst ingredient B (1,0) then A (0,75); pumps P_C02 then P_C01.
        let d=&a.dash;assert_eq!((d.today,d.week),(2,2));assert_eq!(d.today_within_pct,Some(50.));
        assert_eq!(d.worst_ingredients.iter().map(|x|x.name.as_str()).collect::<Vec<_>>(),vec!["B","A"]);
        assert_eq!(d.worst_pumps.iter().map(|x|x.name.as_str()).collect::<Vec<_>>(),vec!["P_C02","P_C01"]);
        // Head filter LH2: nothing (all items on LH1).
        let h=db.analysis(&Filter{head:Some("LH2".into()),..Default::default()}).unwrap();assert!(h.pots.is_empty() && h.ingredients.is_empty() && h.tests.is_empty());
        // Period filter before the data: empty.
        assert!(db.analysis(&Filter{to_ms:Some(now-60_000),..Default::default()}).unwrap().pots.is_empty());
        drop(db);cleanup(&dir);
    }
    #[test]fn day_start_is_local_midnight_also_on_dst_days(){
        let utc=|y:i64,mo:i64,d:i64,h:i64|{let yy=if mo<=2{y-1}else{y};let era=yy.div_euclid(400);let yoe=yy-era*400;let doy=(153*(mo+if mo>2{-3}else{9})+2)/5+d-1;
            ((era*146097+yoe*365+yoe/4-yoe/100+doy-719468)*24+h)*3_600_000};
        // Normal winter day: 3 Nov 2026 12:00 CET (11:00 UTC) → 2 Nov 23:00 UTC.
        assert_eq!(day_start(utc(2026,11,3,11)),utc(2026,11,2,23));
        // Spring forward, 29 Mar 2026: noon CEST = 10:00 UTC; local midnight was still CET = 28 Mar 23:00 UTC.
        assert_eq!(day_start(utc(2026,3,29,10)),utc(2026,3,28,23));assert_eq!(&crate::applog::local_stamp(utc(2026,3,28,23))[..16],"2026-03-29 00:00");
        // Fall back, 25 Oct 2026: noon CET = 11:00 UTC; local midnight was still CEST = 24 Oct 22:00 UTC.
        assert_eq!(day_start(utc(2026,10,25,11)),utc(2026,10,24,22));
        // Summer day and exactly midnight.
        assert_eq!(day_start(utc(2026,7,1,22)),utc(2026,7,1,22));assert_eq!(day_start(utc(2026,7,1,21)),utc(2026,6,30,22));
    }
    #[test]fn worst_lists_only_weighed_items(){
        let now=now_ms();let (dir,db)=fixture("worst",now);let pid=db.profile(ProfileKind::Vsd).unwrap().id;
        // An item with a deviation but no weighed value (e.g. stopped) must not appear among the worst.
        let p:i64=db.conn.query_row("SELECT min(id) FROM pot_run",[],|r|r.get(0)).unwrap();
        db.conn.execute("INSERT INTO pot_item(pot_run_id,step_idx,item_idx,ingredient,pump,head,requested_g,dosed_g,deviation_pct,tolerance_pct,status) VALUES(?1,9,0,'Z','P_C09','LH1',1,NULL,50,1,'te_veel')",[p]).unwrap();
        let d=db.dashboard(pid,&Filter::default(),now).unwrap();
        assert!(!d.worst_ingredients.iter().any(|x|x.name=="Z") && !d.worst_pumps.iter().any(|x|x.name=="P_C09"),"{d:?}");
        drop(db);cleanup(&dir);
    }
    #[test]fn n3_simulation_only_when_switched_on(){
        let now=now_ms();let (dir,db)=fixture("n3",now);
        let a=db.analysis(&Filter{include_sim:true,..Default::default()}).unwrap();
        // A with the simulated +10 %: (0,5 − 1,0 + 10) / 3 = 3,1667; recipe 3 pots, 1 within = 33,3 %; tests 4 doses.
        let ia=a.ingredients.iter().find(|x|x.name=="A").unwrap();assert_eq!(ia.n,3);assert!((ia.mean_pct-9.5/3.).abs()<1e-9);
        assert_eq!((a.recipes[0].pots,a.recipes[0].within),(3,1));assert!((a.recipes[0].within_pct-100./3.).abs()<1e-9);assert_eq!(a.tests.len(),4);
        assert!(a.pots.iter().any(|p|p.simulation));
        // Exports mark it: column "Soort" and the file name.
        let dirx=dir.join("export");let p=export(&a,&dirx,true).unwrap();assert!(p[0].to_string_lossy().contains("met_simulatie"));
        let c=csv_files(&a);assert!(String::from_utf8_lossy(&c[0].1).contains(";simulatie;"));
        drop(db);cleanup(&dir);
    }
    #[test]fn n2_export_xlsx_and_csv_round_trip(){
        let now=now_ms();let (dir,db)=fixture("n2",now);
        let a=db.analysis(&Filter::default()).unwrap();
        let x=crate::xlsx::workbook(&sheets(&a)).unwrap();
        let files=crate::report::tests::unzip(&x);
        for (n,d) in &files{crate::report::tests::well_formed(std::str::from_utf8(d).unwrap()).unwrap_or_else(|e|panic!("{n}: {e}"));}
        let wb=String::from_utf8(files.iter().find(|f|f.0=="xl/workbook.xml").unwrap().1.clone()).unwrap();
        for n in ["Potten","Ingrediënten","Testen","Kalibraties"]{assert!(wb.contains(&format!("name=\"{n}\"")),"{n}");}
        let s2=String::from_utf8(files.iter().find(|f|f.0=="xl/worksheets/sheet2.xml").unwrap().1.clone()).unwrap();
        let sst=crate::xlsx::tests::strings(std::str::from_utf8(&files.iter().find(|f|f.0=="xl/sharedStrings.xml").unwrap().1).unwrap());
        let cells=crate::xlsx::tests::cells(&s2,&sst);let get=|r:&str|cells.iter().find(|c|c.0==r).map(|c|c.1.clone()).unwrap();
        // Row 2 = pot 1 item A: requested 1, dosed 1.005 (numbers as numbers), deviation 0.5.
        assert_eq!((get("G2"),get("J2"),get("K2")),("A".into(),"1".into(),"1.005".into()));assert!((get("L2").parse::<f64>().unwrap()-0.5).abs()<1e-9);
        assert!(s2.contains(r#"<c r="C2" s="1">"#),"datum als datum");
        // CSV NL: BOM, semicolons, decimal comma.
        let c=csv_files(&a);let ing=String::from_utf8(c[1].1.clone()).unwrap();assert!(ing.starts_with('\u{feff}'));
        let line=ing.lines().nth(1).unwrap();assert!(line.contains(";A;P_C01;LH1;1,0000;1,0050;0,5000;"),"{line}");
        // openpyxl (read-only use of the existing venv) when present.
        let py=std::path::Path::new(r"C:\Users\Thomas\CAN\.venv\Scripts\python.exe");
        if py.exists(){
            let f=dir.join("t.xlsx");std::fs::write(&f,&x).unwrap();
            let out=std::process::Command::new(py).env("PYTHONIOENCODING","utf-8").arg("-c").arg("import sys,openpyxl;wb=openpyxl.load_workbook(sys.argv[1]);ws=wb['Ingrediënten'];print(wb.sheetnames);print(ws['G2'].value,ws['K2'].value,round(ws['L2'].value,6),type(ws['C2'].value).__name__,wb['Potten'].max_row)").arg(&f).output().unwrap();
            let s=String::from_utf8_lossy(&out.stdout);assert!(out.status.success(),"{}",String::from_utf8_lossy(&out.stderr));
            assert!(s.contains("['Potten', 'Ingrediënten', 'Testen', 'Kalibraties']") && s.contains("A 1.005 0.5 datetime 3"),"{s}");
            eprintln!("N2 openpyxl: {}",s.trim().replace('\n'," | "));
        }
        drop(db);cleanup(&dir);
    }
    #[test]fn log_retention_deletes_old_rows_only(){
        let dir=temp("logret");let mut db=Db::open(&dir.join("p.db")).unwrap();let now=now_ms();
        let line=|ms:i64|crate::applog::Line{time:crate::applog::local_stamp(ms),level:crate::applog::Level::Info,area:"T".into(),text:format!("r{ms}")};
        db.insert_logs(&[line(now-100*86_400_000),line(now-91*86_400_000),line(now-89*86_400_000),line(now)]).unwrap();
        let before=db.log_count("r").unwrap();assert_eq!(db.prune_log(90,now).unwrap(),2);assert_eq!(db.log_count("r").unwrap(),before-2);
        assert_eq!(db.prune_log(0,now).unwrap(),0,"0 = alles bewaren");
        drop(db);cleanup(&dir);
    }
    /// Realistic size: 10 000 pots × 10 items, 2 000 test doses, 500 calibrations (release; `cargo test --release`).
    /// The debug build uses a tenth of the pots to keep the suite short. Times printed.
    #[test]fn big_database_query_and_export_times(){
        let pots:i64=if cfg!(debug_assertions){1000}else{10_000};
        let dir=temp("big");let mut db=Db::open(&dir.join("p.db")).unwrap();r1_setup(&mut db);let now=now_ms();
        let v=db.save_recipe(ProfileKind::Vsd,None,&crate::recipe::RecipeDraft{name:"Groot".into(),steps:vec![step(vec![item("A",1.)])],..Default::default()}).unwrap().version_id;
        let pid=db.profile(ProfileKind::Vsd).unwrap().id;let t=std::time::Instant::now();
        {let tx=db.conn.transaction().unwrap();
            {let mut pr=tx.prepare("INSERT INTO pot_run(profile_id,recipe_version_id,simulation,mode,status,started_ms,barcode) VALUES(?1,?2,0,'productie','klaar',?3,?4)").unwrap();
             let mut it=tx.prepare("INSERT INTO pot_item(pot_run_id,step_idx,item_idx,ingredient,pump,head,requested_g,dosed_g,deviation_pct,tolerance_pct,status) VALUES(?1,?2,0,?3,?4,?5,1,?6,?7,1,?8)").unwrap();
             for k in 0..pots{pr.execute(params![pid,v,now-k*60_000,format!("{k:010}")]).unwrap();let id=tx.last_insert_rowid();
                for s in 0..10i64{let d=((k*7+s*13)%40-20) as f64/10.;it.execute(params![id,s,["A","B","C","D","E"][(s%5) as usize],format!("P_C{:02}",s+1),if s<5{"LH1"}else{"LH2"},1.+d/100.,d,if d.abs()<=1.{"ok"}else if d<0.{"te_weinig"}else{"te_veel"}]).unwrap();}}}
            {tx.execute("INSERT INTO test_run(profile_id,pump_name,simulation,mode,tolerance_pct,status,started_ms) VALUES(?1,'P_C01',0,'reeks',1,'klaar',?2)",params![pid,now]).unwrap();let run=tx.last_insert_rowid();
             let mut d=tx.prepare("INSERT INTO test_dose(run_id,nr,requested_g,dosed_g,status,time_ms) VALUES(?1,?2,1,?3,'ok',?4)").unwrap();
             for k in 0..2000i64{d.execute(params![run,k,1.+(k%7) as f64/1000.,now-k*1000]).unwrap();}}
            {let mut c=tx.prepare("INSERT INTO calibration(profile_id,pump_name,date_ms,kind,simulation,g_per_stroke,status) VALUES(?1,?2,?3,'vsd',0,0.5,'archief')").unwrap();
             for k in 0..500i64{c.execute(params![pid,format!("P_C{:02}",k%48+1),now-k*86_400_000]).unwrap();}}
            tx.commit().unwrap();}
        let fill=t.elapsed();
        let t=std::time::Instant::now();let a=db.analysis(&Filter::default()).unwrap();let q=t.elapsed();
        assert_eq!((a.pots.len() as i64,a.items.len() as i64,a.tests.len(),a.cals.len()),(pots,pots*10,2000,500));
        let t=std::time::Instant::now();let x=crate::xlsx::workbook(&sheets(&a)).unwrap();let xt=t.elapsed();
        let t=std::time::Instant::now();let c=csv_files(&a);let ct=t.elapsed();
        let t=std::time::Instant::now();let w=db.analysis(&Filter{from_ms:Some(now-7*86_400_000),..Default::default()}).unwrap();let wq=t.elapsed();
        eprintln!("Grote DB (vullen {fill:?}): analyse alles {q:?} ({} potten, {} items), 7 dagen {wq:?} ({} potten); xlsx {xt:?} ({} kB), csv {ct:?} ({} kB)",
            a.pots.len(),a.items.len(),w.pots.len(),x.len()/1024,c.iter().map(|f|f.1.len()).sum::<usize>()/1024);
        drop(db);cleanup(&dir);
    }
}
