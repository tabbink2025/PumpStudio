//! Short management report (Word .docx) for a dose run: summary, per-weight table and native Word charts.
//! Written without extra crates: stored (uncompressed) ZIP plus hand-written OOXML. Charts carry literal data, no embedded workbook.
use crate::{model::*,workflow::*};
use std::{fmt::Write as _,path::PathBuf};

const ACCENT:&str="145AAA";const MUTED:&str="6C7684";const WARN:&str="C48012";const INK:&str="1C2430";
// A4 with 2 cm margins: 17 cm text width.
const TEXT_EMU:i64=6_120_000;const CHART_EMU:i64=2_700_000;

struct Dose{nr:usize,requested:f64,mass:f64,err_g:f64,err_pct:f64}
struct Group{label:String,requested:f64,n:usize,mass:f64,err_g:f64,err_pct:f64,sd_pct:f64,max_pct:f64}

/// Report path next to the run's JSON/CSV: `<run>_rapport.docx`.
pub fn report_path(state:&State)->Option<PathBuf>{
    let p=PathBuf::from(&state.path);let stem=p.file_stem()?.to_str()?.to_string();
    if state.path.is_empty(){return None;}Some(p.with_file_name(format!("{stem}_rapport.docx")))
}

pub fn dose_report(state:&State)->Result<PathBuf>{
    let path=report_path(state).ok_or("Geen meetbestand om rapport naast te bewaren")?;
    let bytes=build(state)?;
    let temp=path.with_extension("docx.tmp");
    std::fs::write(&temp,bytes).map_err(|e|e.to_string())?;
    std::fs::rename(&temp,&path).map_err(|e|{let _=std::fs::remove_file(&temp);format!("Rapport niet opgeslagen (geopend in Word?): {e}")})?;
    Ok(path)
}

pub fn build(state:&State)->Result<Vec<u8>>{
    let doses:Vec<Dose>=state.records.iter().filter(|r|r.kind=="Dosering").filter_map(|r|Some(Dose{nr:r.repeat,requested:r.requested_g?,mass:r.mass_g?,
        err_g:r.error_g.unwrap_or(r.mass_g?-r.requested_g?),err_pct:r.error_pct.unwrap_or(100.*(r.mass_g?-r.requested_g?)/r.requested_g?)})).collect();
    if doses.is_empty(){return Err("Geen gemeten doses voor rapport".into());}
    let failed=state.records.iter().filter(|r|r.kind=="Dosering" && r.status!="completed").count();
    let mixed=doses.iter().any(|d|d.requested!=doses[0].requested);
    let n=doses.len();
    let req:Vec<f64>=doses.iter().map(|d|d.requested).collect();let mass:Vec<f64>=doses.iter().map(|d|d.mass).collect();
    let eg:Vec<f64>=doses.iter().map(|d|d.err_g).collect();let ep:Vec<f64>=doses.iter().map(|d|d.err_pct).collect();
    let (req_mean,_)=stats(&req);let (mass_mean,mass_sd)=stats(&mass);let (eg_mean,eg_sd)=stats(&eg);let (ep_mean,ep_sd)=stats(&ep);
    let worst=doses.iter().max_by(|a,b|a.err_pct.abs().total_cmp(&b.err_pct.abs())).unwrap();
    let within=|lim:f64|ep.iter().filter(|e|e.abs()<=lim).count();
    let (lo,hi)=req.iter().fold((f64::MAX,f64::MIN),|(a,b),g|(a.min(*g),b.max(*g)));
    let correction=state.records.iter().find_map(|r|r.correction_pct).unwrap_or(0.);
    let cfg=&state.config;

    let mut body=String::new();
    p(&mut body,"Title","Doseerrapport");
    p(&mut body,"Subtitle",&format!("{} · {} · pomp {} · {}",date_nl(state.started_ms),if mixed{"doseertest gemengde gewichten"}else{"dosering vast gewicht"},cfg.pump_type,cfg.fluid));
    if state.simulation{p_color(&mut body,"Normal","SIMULATIE: geen echte pomp of weegschaal. Alleen ter controle van werkwijze en rapport.","B92828",true);}

    h(&mut body,"Samenvatting");
    let mut s=format!("{n} doses gemeten");
    if mixed{let _=write!(s," tussen {} en {} g",nl(lo,3),nl(hi,3));}else{let _=write!(s," van {} g",nl(req[0],3));}
    let _=write!(s,". Gemiddeld gevraagd {} g, gemiddeld gedoseerd {} g. Gemiddelde afwijking {} g ({} %), spreiding {} %. ",
        nl(req_mean,4),nl(mass_mean,4),nls(eg_mean,4),nls(ep_mean,2),nl(ep_sd,2));
    let _=write!(s,"{} van {n} doses binnen ±1 %; grootste afwijking {} % bij dosis {}.",within(1.),nls(worst.err_pct,2),worst.nr);
    if correction!=0.{let _=write!(s," Handmatige correctie {} % toegepast.",nls(correction,2));}
    if failed>0{let _=write!(s," Let op: {failed} dosis/doses niet afgerond; reeks onvolledig.");}
    p(&mut body,"Normal",&s);
    let summary=vec![
        vec!["Aantal doses".into(),if failed>0{format!("{n} gemeten, {failed} niet afgerond")}else{n.to_string()}],
        vec!["Gevraagd gewicht".into(),if mixed{format!("{} – {} g, gemiddeld {} g",nl(lo,3),nl(hi,3),nl(req_mean,4))}else{format!("{} g per dosis",nl(req[0],4))}],
        vec!["Gedoseerd gewicht, gemiddeld".into(),if mixed{format!("{} g",nl(mass_mean,4))}else{format!("{} g (spreiding {} g)",nl(mass_mean,4),nl(mass_sd,4))}],
        vec!["Gemiddelde afwijking".into(),format!("{} g  ({} %)",nls(eg_mean,4),nls(ep_mean,2))],
        vec!["Spreiding afwijking (SD)".into(),format!("{} g  ({} %)",nl(eg_sd,4),nl(ep_sd,2))],
        vec!["Grootste afwijking".into(),format!("{} g  ({} %) bij dosis {}",nls(worst.err_g,4),nls(worst.err_pct,2),worst.nr)],
        vec!["Binnen ±1 % / ±2 %".into(),format!("{} / {} van {n}",within(1.),within(2.))],
        vec!["Totaal gevraagd / gedoseerd".into(),format!("{} g / {} g",nl(req.iter().sum(),3),nl(mass.iter().sum(),3))],
    ];
    table(&mut body,&[3600,6038],None,&summary,&[false,false],true);

    let groups=groups(&doses);
    h(&mut body,if groups.iter().any(|g|g.label.contains('–')){"Resultaten per gewichtsklasse"}else{"Resultaten per gewicht"});
    let rows:Vec<Vec<String>>=groups.iter().map(|g|vec![g.label.clone(),g.n.to_string(),nl(g.requested,4),nl(g.mass,4),nls(g.err_g,4),nls(g.err_pct,2),nl(g.sd_pct,2),nl(g.max_pct,2)]).collect();
    table(&mut body,&[1700,600,1150,1150,1150,1150,1150,1588],
        Some(&["Gevraagd","n","Gevraagd gem. (g)","Gedoseerd gem. (g)","Afwijking (g)","Afwijking (%)","Spreiding (%)","Max |afw.| (%)"]),&rows,&[false,true,true,true,true,true,true,true],false);

    h(&mut body,"Grafieken");
    let mut charts=vec![];
    let xs:Vec<f64>=doses.iter().map(|d|d.nr as f64).collect();
    let ys_all:Vec<f64>=req.iter().chain(&mass).copied().collect();
    let (ymin,ymax)=pad(&ys_all);
    charts.push(scatter("Dosis","Gewicht (g)",&nice_axis(0.,n.max(2) as f64+0.5,1.),&nice_axis(ymin,ymax,0.),&[
        Ser{name:"Gevraagd",x:&xs,y:&req,color:MUTED,line:!mixed,marker:if mixed{"square"}else{"none"}},
        Ser{name:"Gedoseerd",x:&xs,y:&mass,color:ACCENT,line:false,marker:"circle"}]));
    chart_par(&mut body,charts.len(),"Gevraagd en gedoseerd gewicht per dosis");
    charts.push(bars(&doses));
    chart_par(&mut body,charts.len(),"Afwijking per dosis (%) — blauw te veel, oranje te weinig");
    if mixed{
        charts.push(scatter("Gevraagd gewicht (g)","Afwijking (%)",&nice_axis(0.,hi,0.),&zero_axis(&ep),&[Ser{name:"Afwijking",x:&req,y:&ep,color:ACCENT,line:false,marker:"circle"}]));
        chart_par(&mut body,charts.len(),"Afwijking tegen gevraagd gewicht");
    }

    h(&mut body,"Testopzet");
    let curve=state.curve.as_ref().map(|c|format!("{} g/omw, {} herhalingen, {} meetpunten",nl(c.yield_g(),4),c.runs.len(),c.steps.len()-1)).unwrap_or("curve na stop of fout niet meer actief".into());
    let first=state.records.iter().find(|r|r.kind=="Dosering");let last=state.records.iter().rev().find(|r|r.kind=="Dosering");
    let setup=vec![
        vec!["Pomp".into(),format!("{} · {} stappen/omw{}",cfg.pump_type,cfg.steps_rev,if cfg.stroke_setting.trim().is_empty(){String::new()}else{format!(" · slaginstelling {}",cfg.stroke_setting.trim())})],
        vec!["Vloeistof".into(),cfg.fluid.clone()],
        vec!["Doseermethode".into(),format!("vanuit 360° meetcurve ({curve})")],
        vec!["Correctie".into(),if correction==0.{"geen".into()}else{format!("{} %",nls(correction,2))}],
        vec!["Weging".into(),format!("stabiel binnen {} g over {} s, uitlooptijd {} s",nl(cfg.range_g,3),nl(cfg.window_s,1),nl(cfg.settle_s,1))],
        vec!["Positieteller".into(),match(first,last){(Some(f),Some(l))=>format!("{} → {}",f.start_steps,l.end_steps.unwrap_or(l.target_steps)),_=>"—".into()}],
        vec!["Bron".into(),format!("{} (+ .csv)",PathBuf::from(&state.path).file_name().and_then(|f|f.to_str()).unwrap_or(""))],
    ];
    table(&mut body,&[3600,6038],None,&setup,&[false,false],true);
    p(&mut body,"Caption","Curve beschrijft gemeten uitstroom; teller registreert verzonden doelen, geen gemeten aspositie.");

    if n<=40{
        h(&mut body,"Bijlage: alle doses");
        let rows:Vec<Vec<String>>=doses.iter().map(|d|vec![d.nr.to_string(),nl(d.requested,4),nl(d.mass,4),nls(d.err_g,4),nls(d.err_pct,2)]).collect();
        table(&mut body,&[1000,2159,2159,2160,2160],Some(&["Dosis","Gevraagd (g)","Gedoseerd (g)","Afwijking (g)","Afwijking (%)"]),&rows,&[true,true,true,true,true],false);
    }else{p(&mut body,"Caption",&format!("Alle {n} doses staan in het CSV-bestand naast dit rapport."));}

    Ok(zip(&package(&body,&charts)))
}


// ---------- Pump test report (iteration 3) ----------
use crate::{pumptest::{self,DoseRow,WeightStats},testdb::RunRow};

/// Word report for one or more test runs of one pump, in the layout of the hand-made reports: introduction,
/// table per requested weight, conclusion/advice as an empty place to fill in, appendix with chart and all
/// doses. Simulation runs say SIMULATIE in the title; simulation and hardware runs are never mixed.
pub fn test_report(runs:&[(RunRow,Vec<DoseRow>)],calibration:Option<&str>)->Result<Vec<u8>>{
    if runs.is_empty(){return Err("Kies minimaal één test".into());}
    let pump=&runs[0].0.pump;
    if runs.iter().any(|(r,_)|&r.pump!=pump){return Err("Alleen testen van dezelfde pomp samen in één verslag".into());}
    let sim=runs[0].0.simulation;
    if runs.iter().any(|(r,_)|r.simulation!=sim){return Err("Simulatie- en hardwaretesten niet samen in één verslag".into());}
    let tol=runs[0].0.tolerance_pct;
    let all:Vec<DoseRow>=runs.iter().flat_map(|(_,d)|d.iter().cloned()).collect();
    let counted:Vec<&DoseRow>=all.iter().filter(|d|d.counts()).collect();
    if counted.is_empty(){return Err("Geen gemeten doses om te rapporteren".into());}
    let stats=pumptest::summary(&all,tol);let total=pumptest::overall(&all,tol);
    let excluded:Vec<&DoseRow>=all.iter().filter(|d|d.excluded).collect();
    let failed=all.iter().filter(|d|d.dosed_g.is_none()).count();
    let tl=pumptest::nl(tol,if tol.fract()==0.{0}else{1});
    let first=runs.iter().map(|(r,_)|r.started_ms).min().unwrap_or(0);let last=runs.iter().map(|(r,_)|r.started_ms).max().unwrap_or(0);
    let mut body=String::new();
    p(&mut body,"Title",&format!("Doseertest {pump}{}",if sim{" — SIMULATIE"}else{""}));
    p(&mut body,"Subtitle",&format!("{}{} · {} test{} · {} doses",date_nl(first as u128).get(..10).unwrap_or(""),
        if last/86_400_000!=first/86_400_000{format!(" – {}",date_nl(last as u128).get(..10).unwrap_or(""))}else{String::new()},runs.len(),if runs.len()==1{""}else{"en"},all.len()));
    if sim{p_color(&mut body,"Normal","SIMULATIE: geen echte pomp of weegschaal. Alleen ter controle van werkwijze en verslag.","B92828",true);}

    h(&mut body,"Inleiding");
    p(&mut body,"Normal",&format!("Doel van deze test is vaststellen hoe nauwkeurig pomp {pump} een gevraagd gewicht doseert."));
    let mut setup=vec![];
    for (r,d) in runs{
        let plan=r.plan();
        let what=match &plan{
            Some(pl)=>match pl.mode{
                pumptest::Mode::Enkel=>format!("één gewicht ({} g) × {}",pumptest::nl(pl.weights.first().copied().unwrap_or(0.),3),pl.repeats),
                pumptest::Mode::Reeks=>format!("reeks {} g × {}{}",pl.weights.iter().map(|w|pumptest::nl(*w,3)).collect::<Vec<_>>().join("; "),pl.repeats,if pl.shuffle{format!(", door elkaar (zaad {})",pl.seed)}else{", oplopend".into()}),
                pumptest::Mode::Willekeurig=>format!("{} doses willekeurig tussen {} en {} g (zaad {})",pl.n,pumptest::nl(pl.min,3),pumptest::nl(pl.max,3),pl.seed),
            },
            None=>r.mode.label().into(),
        };
        let corr:Vec<f64>={let mut c:Vec<f64>=d.iter().map(|x|x.correction_pct).collect();c.sort_by(f64::total_cmp);c.dedup();c};
        let corr_text=match corr.as_slice(){[]=>"—".into(),[c]=>format!("{} %",pumptest::nls(*c,1)),cs=>format!("{} … {} % (gewijzigd tijdens de test)",pumptest::nls(cs[0],1),pumptest::nls(*cs.last().unwrap(),1))};
        setup.push(vec![format!("Test {}",r.id),format!("{} · {what} · correctie {corr_text}{} · {}",date_nl(r.started_ms as u128),
            plan.map(|pl|format!(" · wachttijd {} s{}",pumptest::nl(pl.settle_s,1),if pl.tare_each{""}else{", zonder tarra per dosis"})).unwrap_or_default(),r.status)]);
    }
    setup.push(vec!["Kalibratie".into(),calibration.unwrap_or("actieve kalibratie op het moment van de test").into()]);
    setup.push(vec!["Criterium".into(),format!("binnen ±{tl} % van het gevraagde gewicht")]);
    table(&mut body,&[2000,7638],None,&setup,&[false,false],true);

    h(&mut body,"Resultaten");
    p(&mut body,"Normal",&format!("Tabel 1 geeft per gevraagd gewicht de gemiddelde afwijking, de spreiding (standaardafwijking), de kleinste en grootste afwijking en het aantal doses binnen ±{tl} %."));
    let row=|w:&WeightStats|vec![w.label.clone(),w.n.to_string(),format!("{} %",pumptest::nls(w.mean_err_pct,2)),format!("{} %",pumptest::nl(w.sd_pct,2)),
        if w.n>0{format!("{} % / {} %",pumptest::nls(w.min_pct,2),pumptest::nls(w.max_pct,2))}else{"—".into()},format!("{}/{}",w.within,w.n),w.g_per_min.map(|g|pumptest::nl(g,1)).unwrap_or("—".into())];
    let mut rows:Vec<Vec<String>>=stats.iter().map(row).collect();
    if let Some(t)=&total{if stats.len()>1{rows.push(row(t));}}
    let within_head=format!("Binnen ±{tl} %");
    table(&mut body,&[1500,600,1450,1300,2200,1300,1288],Some(&["Gevraagd gewicht","n","Gemiddelde afwijking","Spreiding (SD)","Kleinste / grootste afwijking",&within_head,"g/min"]),&rows,&[false,true,true,true,true,true,true],false);
    let mut caption="Tabel 1. Afwijking ten opzichte van het gevraagde gewicht.".to_string();
    if !excluded.is_empty(){
        let mut reasons:Vec<(String,usize)>=vec![];
        for d in &excluded{match reasons.iter_mut().find(|r|r.0==d.exclude_reason){Some(r)=>r.1+=1,None=>reasons.push((d.exclude_reason.clone(),1))}}
        let _=write!(caption," {} dosis/doses niet meegenomen: {}.",excluded.len(),reasons.iter().map(|(r,n)|format!("{n}× {r}")).collect::<Vec<_>>().join("; "));
    }
    if failed>0{let _=write!(caption," {failed} dosis/doses niet gemeten (gestopt of fout).");}
    p(&mut body,"Caption",&caption);
    // Small doses: spread as an indication of the smallest useful dose (ceramic drop size).
    let small:Vec<&WeightStats>=stats.iter().filter(|w|w.requested<=5. && w.n>=2).collect();
    if small.len()>=2{
        let mut r:Vec<Vec<String>>=small.iter().map(|w|vec![w.label.clone(),format!("{} g",pumptest::nl(w.sd_g,4)),format!("{} %",pumptest::nl(w.sd_pct,2))]).collect();
        r.truncate(12);
        table(&mut body,&[2600,3500,3538],Some(&["Gevraagd gewicht","Spreiding (g)","Spreiding (%)"]),&r,&[false,true,true],false);
        p(&mut body,"Caption",&format!("Tabel 2. Spreiding bij kleine doses (indicatie druppelgrootte).{}",
            pumptest::smallest_within(&stats,tol).map(|g|format!(" Kleinste geteste dosis met spreiding binnen ±{tl} %: {} g.",pumptest::nl(g,3))).unwrap_or_default()));
    }

    h(&mut body,"Conclusie en advies");
    p_color(&mut body,"Normal","[In te vullen: eigen conclusie en advies.]",MUTED,false);

    h(&mut body,"Bijlage: grafische weergave");
    p(&mut body,"Normal",&format!("Per gevraagd gewicht de gemiddelde afwijking (punt) en de kleinste en grootste afwijking. De stippellijnen geven de grens van ±{tl} % aan."));
    let mut charts=vec![];
    let ws:Vec<&WeightStats>=stats.iter().filter(|w|w.n>0).collect();
    let x:Vec<f64>=ws.iter().map(|w|w.requested).collect();
    let mean:Vec<f64>=ws.iter().map(|w|w.mean_err_pct).collect();let lo:Vec<f64>=ws.iter().map(|w|w.min_pct).collect();let hi:Vec<f64>=ws.iter().map(|w|w.max_pct).collect();
    let xmax=x.iter().cloned().fold(0.,f64::max)*1.05+0.1;let band_x=[0.,xmax];let up=[tol,tol];let down=[-tol,-tol];
    let ally:Vec<f64>=lo.iter().chain(&hi).chain(&up).chain(&down).copied().collect();
    let (up_name,down_name)=(format!("+{tl} %"),format!("−{tl} %"));
    charts.push(scatter("Gevraagd gewicht (g)","Afwijking (%)",&nice_axis(0.,xmax,0.),&zero_axis(&ally),&[
        Ser{name:"Gemiddeld",x:&x,y:&mean,color:ACCENT,line:false,marker:"circle"},
        Ser{name:"Kleinste",x:&x,y:&lo,color:WARN,line:false,marker:"square"},
        Ser{name:"Grootste",x:&x,y:&hi,color:WARN,line:false,marker:"square"},
        Ser{name:&up_name,x:&band_x,y:&up,color:MUTED,line:true,marker:"none"},
        Ser{name:&down_name,x:&band_x,y:&down,color:MUTED,line:true,marker:"none"}]));
    chart_par(&mut body,charts.len(),"Afwijking ten opzichte van het gevraagde gewicht (zelfde gegevens als tabel 1).");
    let per:Vec<Dose>=counted.iter().enumerate().map(|(k,d)|Dose{nr:k+1,requested:d.requested_g,mass:d.dosed_g.unwrap_or(0.),err_g:d.err_g().unwrap_or(0.),err_pct:d.err_pct().unwrap_or(0.)}).collect();
    charts.push(bars(&per));
    chart_par(&mut body,charts.len(),"Afwijking per dosis (%), in volgorde van doseren — blauw te veel, oranje te weinig.");

    h(&mut body,"Bijlage: alle doses");
    let rows:Vec<Vec<String>>=runs.iter().flat_map(|(r,d)|d.iter().map(move|x|(r.id,x))).map(|(rid,d)|vec![rid.to_string(),d.nr.to_string(),pumptest::nl(d.requested_g,3),
        d.dosed_g.map(|g|pumptest::nl(g,4)).unwrap_or("—".into()),d.err_pct().map(|e|pumptest::nls(e,2)).unwrap_or("—".into()),pumptest::nls(d.correction_pct,1),
        if d.excluded{format!("uitgesloten: {}",d.exclude_reason)}else if d.dosed_g.is_none(){format!("niet gemeten: {}",d.error)}else{String::new()}]).collect();
    table(&mut body,&[700,600,1300,1400,1300,1200,3138],Some(&["Test","Nr","Gevraagd (g)","Gedoseerd (g)","Afwijking (%)","Correctie (%)","Opmerking"]),&rows,&[true,true,true,true,true,true,false],false);
    Ok(zip(&package(&body,&charts)))
}
/// Save next to the other results: `pomptest_<pomp>_<runs>.docx`.
pub fn save_test_report(dir:&std::path::Path,pump:&str,run_ids:&[i64],bytes:&[u8])->Result<PathBuf>{
    std::fs::create_dir_all(dir).map_err(|e|e.to_string())?;
    let ids=run_ids.iter().map(|i|i.to_string()).collect::<Vec<_>>().join("-");
    let path=dir.join(format!("pomptest_{}_{}.docx",pump.replace(['/','\\',':',' '],"_"),ids));
    let temp=path.with_extension("docx.tmp");
    std::fs::write(&temp,bytes).map_err(|e|e.to_string())?;
    std::fs::rename(&temp,&path).map_err(|e|{let _=std::fs::remove_file(&temp);format!("Verslag niet opgeslagen (geopend in Word?): {e}")})?;
    Ok(path)
}

fn groups(doses:&[Dose])->Vec<Group>{
    let mut weights:Vec<f64>=doses.iter().map(|d|d.requested).collect();weights.sort_by(f64::total_cmp);weights.dedup();
    let make=|label:String,set:Vec<&Dose>|{
        let r:Vec<f64>=set.iter().map(|d|d.requested).collect();let m:Vec<f64>=set.iter().map(|d|d.mass).collect();
        let g:Vec<f64>=set.iter().map(|d|d.err_g).collect();let p:Vec<f64>=set.iter().map(|d|d.err_pct).collect();
        let (sp,sd)=stats(&p);
        Group{label,requested:stats(&r).0,n:set.len(),mass:stats(&m).0,err_g:stats(&g).0,err_pct:sp,sd_pct:sd,max_pct:p.iter().fold(0.,|a:f64,b|a.max(b.abs()))}
    };
    if weights.len()<=12{
        return weights.iter().map(|w|make(format!("{} g",nl(*w,3)),doses.iter().filter(|d|d.requested==*w).collect())).collect();
    }
    // Random tests: five equal-width weight classes.
    let (lo,hi)=(weights[0],*weights.last().unwrap());let step=(hi-lo)/5.;
    (0..5).filter_map(|i|{
        let a=lo+step*i as f64;let b=if i==4{hi}else{lo+step*(i+1) as f64};
        let set:Vec<&Dose>=doses.iter().filter(|d|d.requested>=a && (d.requested<b || (i==4 && d.requested<=b))).collect();
        if set.is_empty(){None}else{Some(make(format!("{} – {} g",nl(a,3),nl(b,3)),set))}
    }).collect()
}

fn span(values:&[f64])->(f64,f64){values.iter().fold((f64::MAX,f64::MIN),|(a,b),v|(a.min(*v),b.max(*v)))}
fn pad(values:&[f64])->(f64,f64){
    let (lo,hi)=span(values);
    let margin=((hi-lo)*0.15).max(hi.abs()*0.005).max(1e-4);
    ((lo-margin).max(0.),hi+margin)
}
/// Axis rounded to a 1-2-2.5-5 step so tick labels are round numbers.
struct Axis{lo:f64,hi:f64,step:f64,fmt:String}
fn nice_axis(lo:f64,hi:f64,min_step:f64)->Axis{
    let raw=((hi-lo).abs()/6.).max(1e-6);let mag=10f64.powf(raw.log10().floor());let f=raw/mag;
    let step=((if f<=1.{1.}else if f<=2.{2.}else if f<=2.5{2.5}else if f<=5.{5.}else{10.})*mag).max(min_step);
    let lo2=(lo/step+1e-9).floor()*step;let mut hi2=(hi/step-1e-9).ceil()*step;if hi2<=lo2{hi2=lo2+step;}
    let mut d=(-step.log10().floor()).max(0.) as usize;if ((step*10f64.powi(d as i32))-(step*10f64.powi(d as i32)).round()).abs()>1e-6{d+=1;}
    Axis{lo:lo2,hi:hi2,step,fmt:if d==0{"0".into()}else{format!("0.{}","0".repeat(d))}}
}
/// Percent axis that always includes zero.
fn zero_axis(values:&[f64])->Axis{let (lo,hi)=span(values);let (lo,hi)=(lo.min(0.),hi.max(0.));if hi-lo<1e-9{nice_axis(-0.1,0.1,0.)}else{nice_axis(lo,hi,0.)}}

// ---------- Text helpers ----------
fn nl(v:f64,d:usize)->String{format!("{v:.d$}").replace('.',",")}
fn nls(v:f64,d:usize)->String{format!("{v:+.d$}").replace('.',",")}
fn esc(s:&str)->String{s.replace('&',"&amp;").replace('<',"&lt;").replace('>',"&gt;").replace('"',"&quot;")}

/// Local date/time for the Netherlands (CET/CEST, EU switch rule) from epoch milliseconds.
fn date_nl(ms:u128)->String{
    let secs=(ms/1000) as i64;
    let year=civil(secs).0;
    // EU summer time: last Sunday of March 01:00 UTC until last Sunday of October 01:00 UTC.
    let last_sunday=|month:i64|{let days=days_from_civil(year,month,31);days-(days+4).rem_euclid(7)};
    let summer=secs>=last_sunday(3)*86400+3600 && secs<last_sunday(10)*86400+3600;
    let (y,mo,d,hh,mm)=civil(secs+if summer{7200}else{3600});
    format!("{d:02}-{mo:02}-{y} {hh:02}:{mm:02}")
}
fn days_from_civil(y:i64,m:i64,d:i64)->i64{let y=if m<=2{y-1}else{y};let era=y.div_euclid(400);let yoe=y-era*400;
    let doy=(153*(m+if m>2{-3}else{9})+2)/5+d-1;let doe=yoe*365+yoe/4-yoe/100+doy;era*146097+doe-719468}
fn civil(secs:i64)->(i64,i64,i64,i64,i64){
    let z=secs.div_euclid(86400)+719468;let rem=secs.rem_euclid(86400);
    let era=z.div_euclid(146097);let doe=z-era*146097;let yoe=(doe-doe/1460+doe/36524-doe/146096)/365;
    let doy=doe-(365*yoe+yoe/4-yoe/100);let mp=(5*doy+2)/153;let d=doy-(153*mp+2)/5+1;let m=if mp<10{mp+3}else{mp-9};
    (yoe+era*400+if m<=2{1}else{0},m,d,rem/3600,rem%3600/60)
}

// ---------- WordprocessingML ----------
fn p(out:&mut String,style:&str,text:&str){let _=write!(out,r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr><w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,esc(text));}
fn p_color(out:&mut String,style:&str,text:&str,color:&str,bold:bool){let _=write!(out,r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr><w:r><w:rPr>{}<w:color w:val="{color}"/></w:rPr><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,if bold{"<w:b/>"}else{""},esc(text));}
fn h(out:&mut String,text:&str){p(out,"Heading1",text);}
fn cell(out:&mut String,width:i64,text:&str,right:bool,bold:bool,shade:Option<&str>){
    let _=write!(out,r#"<w:tc><w:tcPr><w:tcW w:w="{width}" w:type="dxa"/>{}</w:tcPr><w:p><w:pPr><w:spacing w:before="30" w:after="30"/>{}</w:pPr><w:r>{}<w:t xml:space="preserve">{}</w:t></w:r></w:p></w:tc>"#,
        shade.map(|c|format!(r#"<w:shd w:val="clear" w:color="auto" w:fill="{c}"/>"#)).unwrap_or_default(),
        if right{r#"<w:jc w:val="right"/>"#}else{""},if bold{"<w:rPr><w:b/></w:rPr>"}else{""},esc(text));
}
fn table(out:&mut String,widths:&[i64],header:Option<&[&str]>,rows:&[Vec<String>],right:&[bool],label_col:bool){
    let line=r#"w:val="single" w:sz="4" w:space="0" w:color="D5DBE3""#;
    let _=write!(out,r#"<w:tbl><w:tblPr><w:tblW w:w="{}" w:type="dxa"/><w:tblBorders><w:top {line}/><w:bottom {line}/><w:insideH {line}/></w:tblBorders><w:tblLayout w:type="fixed"/><w:tblCellMar><w:left w:w="80" w:type="dxa"/><w:right w:w="80" w:type="dxa"/></w:tblCellMar></w:tblPr><w:tblGrid>"#,widths.iter().sum::<i64>());
    for w in widths{let _=write!(out,r#"<w:gridCol w:w="{w}"/>"#);}
    out.push_str("</w:tblGrid>");
    if let Some(head)=header{
        out.push_str(r#"<w:tr><w:trPr><w:tblHeader/></w:trPr>"#);
        for (i,t) in head.iter().enumerate(){cell(out,widths[i],t,right[i],true,Some("EEF2F7"));}
        out.push_str("</w:tr>");
    }
    for row in rows{
        out.push_str(r#"<w:tr><w:trPr><w:cantSplit/></w:trPr>"#);
        for (i,t) in row.iter().enumerate(){cell(out,widths[i],t,right[i],label_col && i==0,if label_col && i==0{Some("F6F8FB")}else{None});}
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl>");
    p(out,"Normal","");
}
fn chart_par(out:&mut String,n:usize,caption:&str){
    let _=write!(out,r#"<w:p><w:pPr><w:keepNext/><w:spacing w:after="0"/></w:pPr><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="{TEXT_EMU}" cy="{CHART_EMU}"/><wp:effectExtent l="0" t="0" r="0" b="0"/><wp:docPr id="{n}" name="Grafiek {n}"/><wp:cNvGraphicFramePr/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart"><c:chart r:id="rIdChart{n}"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#);
    p(out,"Caption",&format!("Figuur {n}. {caption}"));
}

// ---------- DrawingML charts (literal data) ----------
struct Ser<'a>{name:&'a str,x:&'a [f64],y:&'a [f64],color:&'a str,line:bool,marker:&'a str}
fn num_lit(values:&[f64],fmt:&str)->String{
    let mut s=format!(r#"<c:numLit><c:formatCode>{fmt}</c:formatCode><c:ptCount val="{}"/>"#,values.len());
    for (i,v) in values.iter().enumerate(){let _=write!(s,r#"<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>"#);}
    s+"</c:numLit>"
}
fn opt_lit(values:&[Option<f64>])->String{
    let mut s=format!(r#"<c:numLit><c:formatCode>General</c:formatCode><c:ptCount val="{}"/>"#,values.len());
    for (i,v) in values.iter().enumerate(){if let Some(v)=v{let _=write!(s,r#"<c:pt idx="{i}"><c:v>{v}</c:v></c:pt>"#);}}
    s+"</c:numLit>"
}
fn fill(color:&str)->String{format!(r#"<a:solidFill><a:srgbClr val="{color}"/></a:solidFill>"#)}
fn rich(text:&str)->String{format!(r#"<c:title><c:tx><c:rich><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="800" b="0"/></a:pPr><a:r><a:rPr lang="nl-NL" sz="800" b="0">{}</a:rPr><a:t>{}</a:t></a:r></a:p></c:rich></c:tx><c:overlay val="0"/></c:title>"#,fill(MUTED),esc(text))}
fn grid()->&'static str{r#"<c:majorGridlines><c:spPr><a:ln w="6350"><a:solidFill><a:srgbClr val="E3E7ED"/></a:solidFill></a:ln></c:spPr></c:majorGridlines>"#}
fn axis_line()->String{format!(r#"<c:spPr><a:ln w="6350">{}</a:ln></c:spPr>"#,fill("B8C0CC"))}
fn val_ax(id:u32,cross:u32,pos:&str,title:&str,gridlines:bool,ax:&Axis,between:&str)->String{
    let (lo,hi,step,fmt)=(ax.lo,ax.hi,ax.step,&ax.fmt);
    format!(r#"<c:valAx><c:axId val="{id}"/><c:scaling><c:orientation val="minMax"/><c:max val="{hi}"/><c:min val="{lo}"/></c:scaling><c:delete val="0"/><c:axPos val="{pos}"/>{}{}<c:numFmt formatCode="{fmt}" sourceLinked="0"/><c:majorTickMark val="out"/><c:minorTickMark val="none"/><c:tickLblPos val="low"/>{}<c:crossAx val="{cross}"/><c:crosses val="autoZero"/><c:crossBetween val="{between}"/><c:majorUnit val="{step}"/></c:valAx>"#,
        if gridlines{grid()}else{""},rich(title),axis_line())
}
fn chart_space(plot:&str,legend:bool)->String{
    format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><c:lang val="nl-NL"/><c:roundedCorners val="0"/><c:chart><c:autoTitleDeleted val="1"/><c:plotArea><c:layout/>{plot}</c:plotArea>{}<c:plotVisOnly val="1"/><c:dispBlanksAs val="gap"/></c:chart><c:spPr><a:noFill/><a:ln><a:noFill/></a:ln></c:spPr><c:txPr><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:defRPr sz="800">{}</a:defRPr></a:pPr><a:endParaRPr lang="nl-NL"/></a:p></c:txPr></c:chartSpace>"#,
        if legend{r#"<c:legend><c:legendPos val="t"/><c:overlay val="0"/></c:legend>"#}else{""},fill(INK))
}
fn scatter(x_title:&str,y_title:&str,x_ax:&Axis,y_ax:&Axis,series:&[Ser])->String{
    let mut plot=r#"<c:scatterChart><c:scatterStyle val="lineMarker"/><c:varyColors val="0"/>"#.to_string();
    for (i,s) in series.iter().enumerate(){
        let line=if s.line{format!(r#"<a:ln w="15875">{}<a:prstDash val="dash"/></a:ln>"#,fill(s.color))}else{r#"<a:ln w="25400"><a:noFill/></a:ln>"#.into()};
        let marker=if s.marker=="none"{r#"<c:marker><c:symbol val="none"/></c:marker>"#.to_string()}
            else if s.marker=="square"{format!(r#"<c:marker><c:symbol val="square"/><c:size val="6"/><c:spPr><a:noFill/><a:ln w="9525">{}</a:ln></c:spPr></c:marker>"#,fill(s.color))}
            else{format!(r#"<c:marker><c:symbol val="circle"/><c:size val="5"/><c:spPr>{}<a:ln w="6350">{}</a:ln></c:spPr></c:marker>"#,fill(s.color),fill(s.color))};
        let _=write!(plot,r#"<c:ser><c:idx val="{i}"/><c:order val="{i}"/><c:tx><c:v>{}</c:v></c:tx><c:spPr>{line}</c:spPr>{marker}<c:xVal>{}</c:xVal><c:yVal>{}</c:yVal><c:smooth val="0"/></c:ser>"#,
            esc(s.name),num_lit(s.x,"General"),num_lit(s.y,"General"));
    }
    plot.push_str(r#"<c:axId val="101"/><c:axId val="102"/></c:scatterChart>"#);
    plot+=&val_ax(101,102,"b",x_title,false,x_ax,"midCat");
    plot+=&val_ax(102,101,"l",y_title,true,y_ax,"midCat");
    chart_space(&plot,series.len()>1)
}
fn bars(doses:&[Dose])->String{
    let mut cats=format!(r#"<c:strLit><c:ptCount val="{}"/>"#,doses.len());
    for (i,d) in doses.iter().enumerate(){let _=write!(cats,r#"<c:pt idx="{i}"><c:v>{}</c:v></c:pt>"#,d.nr);}
    cats.push_str("</c:strLit>");
    let pos:Vec<Option<f64>>=doses.iter().map(|d|(d.err_pct>=0.).then_some(d.err_pct)).collect();
    let neg:Vec<Option<f64>>=doses.iter().map(|d|(d.err_pct<0.).then_some(d.err_pct)).collect();
    let mut plot=r#"<c:barChart><c:barDir val="col"/><c:grouping val="clustered"/><c:varyColors val="0"/>"#.to_string();
    for (i,(name,color,vals)) in [("Te veel",ACCENT,&pos),("Te weinig",WARN,&neg)].into_iter().enumerate(){
        let _=write!(plot,r#"<c:ser><c:idx val="{i}"/><c:order val="{i}"/><c:tx><c:v>{name}</c:v></c:tx><c:spPr>{}</c:spPr><c:invertIfNegative val="0"/><c:cat>{cats}</c:cat><c:val>{}</c:val></c:ser>"#,fill(color),opt_lit(vals));
    }
    let gap=if doses.len()>60{20}else{60};
    let _=write!(plot,r#"<c:gapWidth val="{gap}"/><c:overlap val="100"/><c:axId val="201"/><c:axId val="202"/></c:barChart>"#);
    let _=write!(plot,r#"<c:catAx><c:axId val="201"/><c:scaling><c:orientation val="minMax"/></c:scaling><c:delete val="0"/><c:axPos val="b"/>{}<c:numFmt formatCode="General" sourceLinked="0"/><c:majorTickMark val="none"/><c:minorTickMark val="none"/><c:tickLblPos val="low"/>{}<c:crossAx val="202"/><c:crosses val="autoZero"/><c:auto val="1"/><c:lblAlgn val="ctr"/><c:lblOffset val="100"/><c:noMultiLvlLbl val="0"/></c:catAx>"#,rich("Dosis"),axis_line());
    let pct:Vec<f64>=doses.iter().map(|d|d.err_pct).collect();
    plot+=&val_ax(202,201,"l","Afwijking (%)",true,&zero_axis(&pct),"between");
    chart_space(&plot,true)
}

// ---------- Package ----------
fn package(body:&str,charts:&[String])->Vec<(String,Vec<u8>)>{
    let mut files:Vec<(String,Vec<u8>)>=vec![];
    let mut types=r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/docProps/core.xml" ContentType="application/vnd.openxmlformats-package.core-properties+xml"/>"#.to_string();
    let mut rels=r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rIdStyles" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#.to_string();
    for (i,c) in charts.iter().enumerate(){
        let n=i+1;
        let _=write!(types,r#"<Override PartName="/word/charts/chart{n}.xml" ContentType="application/vnd.openxmlformats-officedocument.drawingml.chart+xml"/>"#);
        let _=write!(rels,r#"<Relationship Id="rIdChart{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="charts/chart{n}.xml"/>"#);
        files.push((format!("word/charts/chart{n}.xml"),c.clone().into_bytes()));
    }
    types.push_str("</Types>");rels.push_str("</Relationships>");
    files.insert(0,("[Content_Types].xml".into(),types.into_bytes()));
    files.push(("_rels/.rels".into(),br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties" Target="docProps/core.xml"/></Relationships>"#.to_vec()));
    files.push(("docProps/core.xml".into(),br#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Doseerrapport</dc:title><dc:creator>Pump Studio</dc:creator></cp:coreProperties>"#.to_vec()));
    files.push(("word/_rels/document.xml.rels".into(),rels.into_bytes()));
    files.push(("word/styles.xml".into(),STYLES.as_bytes().to_vec()));
    let document=format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"><w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1134" w:right="1134" w:bottom="1134" w:left="1134" w:header="567" w:footer="567" w:gutter="0"/></w:sectPr></w:body></w:document>"#);
    files.push(("word/document.xml".into(),document.into_bytes()));
    files
}

const STYLES:&str=r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri" w:hAnsi="Calibri" w:eastAsia="Calibri" w:cs="Calibri"/><w:color w:val="1C2430"/><w:sz w:val="20"/><w:szCs w:val="20"/><w:lang w:val="nl-NL"/></w:rPr></w:rPrDefault><w:pPrDefault><w:pPr><w:spacing w:after="100" w:line="264" w:lineRule="auto"/></w:pPr></w:pPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:qFormat/></w:style>
<w:style w:type="paragraph" w:styleId="Title"><w:name w:val="Title"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:spacing w:after="40"/></w:pPr><w:rPr><w:b/><w:color w:val="152133"/><w:sz w:val="44"/><w:szCs w:val="44"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Subtitle"><w:name w:val="Subtitle"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:pBdr><w:bottom w:val="single" w:sz="8" w:space="6" w:color="145AAA"/></w:pBdr><w:spacing w:after="240"/></w:pPr><w:rPr><w:color w:val="6C7684"/><w:sz w:val="20"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:qFormat/><w:pPr><w:keepNext/><w:spacing w:before="280" w:after="100"/><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/><w:color w:val="145AAA"/><w:sz w:val="26"/><w:szCs w:val="26"/></w:rPr></w:style>
<w:style w:type="paragraph" w:styleId="Caption"><w:name w:val="caption"/><w:basedOn w:val="Normal"/><w:qFormat/><w:pPr><w:spacing w:before="40" w:after="200"/></w:pPr><w:rPr><w:i/><w:color w:val="6C7684"/><w:sz w:val="17"/></w:rPr></w:style>
</w:styles>"#;

// ---------- Stored ZIP ----------
pub(crate) fn crc32(data:&[u8])->u32{
    let mut table=[0u32;256];
    for (i,t) in table.iter_mut().enumerate(){let mut c=i as u32;for _ in 0..8{c=if c&1!=0{0xEDB8_8320^(c>>1)}else{c>>1};}*t=c;}
    !data.iter().fold(!0u32,|c,b|table[((c^*b as u32)&0xFF) as usize]^(c>>8))
}
pub(crate) fn zip(files:&[(String,Vec<u8>)])->Vec<u8>{zip_with(files,false)}
/// Zip with DEFLATE (method 8) for large parts (the .xlsx export).
pub(crate) fn zip_deflate(files:&[(String,Vec<u8>)])->Vec<u8>{zip_with(files,true)}
fn zip_with(files:&[(String,Vec<u8>)],compress:bool)->Vec<u8>{
    let mut out=vec![];let mut central=vec![];
    let le16=|v:&mut Vec<u8>,x:u16|v.extend_from_slice(&x.to_le_bytes());
    let le32=|v:&mut Vec<u8>,x:u32|v.extend_from_slice(&x.to_le_bytes());
    for (name,data) in files{
        let offset=out.len() as u32;let crc=crc32(data);let size=data.len() as u32;
        let (method,body)=if compress{(8u16,crate::deflate::deflate(data))}else{(0u16,data.clone())};let csize=body.len() as u32;
        // Fixed DOS timestamp 1980-01-01 00:00.
        le32(&mut out,0x0403_4b50);le16(&mut out,20);le16(&mut out,0);le16(&mut out,method);le16(&mut out,0);le16(&mut out,0x21);
        le32(&mut out,crc);le32(&mut out,csize);le32(&mut out,size);le16(&mut out,name.len() as u16);le16(&mut out,0);
        out.extend_from_slice(name.as_bytes());out.extend_from_slice(&body);
        le32(&mut central,0x0201_4b50);le16(&mut central,20);le16(&mut central,20);le16(&mut central,0);le16(&mut central,method);le16(&mut central,0);le16(&mut central,0x21);
        le32(&mut central,crc);le32(&mut central,csize);le32(&mut central,size);le16(&mut central,name.len() as u16);
        le16(&mut central,0);le16(&mut central,0);le16(&mut central,0);le16(&mut central,0);le32(&mut central,0);le32(&mut central,offset);
        central.extend_from_slice(name.as_bytes());
    }
    let start=out.len() as u32;let size=central.len() as u32;out.extend(central);
    le32(&mut out,0x0605_4b50);le16(&mut out,0);le16(&mut out,0);le16(&mut out,files.len() as u16);le16(&mut out,files.len() as u16);
    le32(&mut out,size);le32(&mut out,start);le16(&mut out,0);out
}

#[cfg(test)] pub(crate) mod tests {
    use super::*;
    /// Read a stored (uncompressed) zip as written by `zip`: name -> bytes. Checks signatures and CRCs.
    pub fn unzip(b:&[u8])->Vec<(String,Vec<u8>)>{
        let mut out=vec![];let mut i=0;
        let u16at=|i:usize|u16::from_le_bytes([b[i],b[i+1]]) as usize;let u32at=|i:usize|u32::from_le_bytes([b[i],b[i+1],b[i+2],b[i+3]]);
        while i+30<=b.len() && u32at(i)==0x0403_4b50{
            let method=u16at(i+8);let crc=u32at(i+14);let size=u32at(i+18) as usize;let n=u16at(i+26);let extra=u16at(i+28);
            let name=String::from_utf8(b[i+30..i+30+n].to_vec()).unwrap();let raw=b[i+30+n+extra..i+30+n+extra+size].to_vec();
            let data=if method==8{crate::deflate::inflate(&raw).unwrap()}else{raw};assert_eq!(data.len(),u32at(i+22) as usize,"{name}");
            assert_eq!(crc32(&data),crc,"{name}");out.push((name,data));i+=30+n+extra+size;
        }
        assert_eq!(u32at(i),0x0201_4b50,"centrale map volgt");out
    }
    /// Minimal XML well-formedness: every start tag closed in order (no DTD, no CDATA in our files).
    pub fn well_formed(x:&str)->std::result::Result<usize,String>{
        let mut stack:Vec<String>=vec![];let mut n=0;let mut rest=x;
        while let Some(p)=rest.find('<'){
            rest=&rest[p+1..];let end=rest.find('>').ok_or("open tag")?;let tag=&rest[..end];rest=&rest[end+1..];
            if tag.starts_with('?') || tag.starts_with('!'){continue;}
            if let Some(name)=tag.strip_prefix('/'){let top=stack.pop().ok_or(format!("sluit {name} zonder open"))?;if top!=name.trim(){return Err(format!("{top} gesloten met {name}"));}}
            else if !tag.ends_with('/'){stack.push(tag.split_whitespace().next().unwrap_or("").to_string());n+=1;}else{n+=1;}
        }
        if stack.is_empty(){Ok(n)}else{Err(format!("niet gesloten: {stack:?}"))}
    }
    fn state(weights:&[f64])->State{
        let mut s=State{path:std::env::temp_dir().join(format!("pump-report-{}.json",std::process::id())).display().to_string(),started_ms:1_793_000_000_000,..Default::default()};
        for (i,g) in weights.iter().enumerate(){let m=g*(1.+0.001*((i%5) as f64-2.));
            s.records.push(Record{kind:"Dosering".into(),repeat:i+1,status:"completed".into(),requested_g:Some(*g),mass_g:Some(m),error_g:Some(m-g),error_pct:Some(100.*(m-g)/g),..Default::default()});}
        s
    }
    #[test]fn axes_are_round(){
        let a=nice_axis(4.925,5.015,0.);assert_eq!((a.lo,a.step,a.fmt.as_str()),(4.92,0.02,"0.00"));assert!((a.hi-5.02).abs()<1e-9);
        let z=zero_axis(&[-1.,-0.99]);assert_eq!(z.hi,0.);assert!(z.lo<=-1.);let x=nice_axis(0.,4.5,1.);assert_eq!((x.step,x.fmt.as_str()),(1.,"0"));
        assert_eq!(nice_axis(0.,1.4,0.).step,0.25);assert_eq!(nice_axis(0.,1.4,0.).fmt,"0.00");
    }
    #[test]fn crc_and_dates(){assert_eq!(crc32(b"123456789"),0xCBF4_3926);assert_eq!(date_nl(1_793_000_000_000),"26-10-2026 08:33");
        assert_eq!(date_nl(1_774_746_000_000),"29-03-2026 03:00");assert_eq!(date_nl(1_774_745_999_000),"29-03-2026 01:59");assert_eq!(date_nl(1_783_000_000_000),"02-07-2026 15:46");}
    #[test]fn report_has_charts_and_classes(){
        let single=build(&state(&[5.;10])).unwrap();let text=String::from_utf8_lossy(&single);
        assert!(single.starts_with(b"PK\x03\x04"));assert!(text.contains("word/charts/chart2.xml") && !text.contains("chart3.xml"));
        assert!(text.contains("5,0000 g per dosis"));
        let weights:Vec<f64>=(0..30).map(|i|0.5+i as f64*0.15).collect();
        let mixed=String::from_utf8_lossy(&build(&state(&weights)).unwrap()).into_owned();
        assert!(mixed.contains("chart3.xml") && mixed.contains("gewichtsklasse") && mixed.contains("Bijlage"));
        assert!(build(&State::default()).is_err());
    }
    fn run(id:i64,sim:bool,doses:&[(f64,f64)])->(RunRow,Vec<DoseRow>){
        let plan=pumptest::TestPlan{mode:pumptest::Mode::Reeks,weights:vec![0.5,1.,5.],repeats:3,..Default::default()};
        (RunRow{id,profile_id:1,pump_id:None,pump:"P_C01".into(),simulation:sim,calibration_id:Some(1),mode:plan.mode,plan:serde_json::to_string(&plan).unwrap(),tolerance_pct:1.,
            status:"klaar".into(),started_ms:1_793_000_000_000,ended_ms:None,note:String::new(),source:String::new()},
         doses.iter().enumerate().map(|(k,(r,m))|DoseRow{run_id:id,nr:k as i64+1,requested_g:*r,dosed_g:Some(*m),status:"ok".into(),duration_s:Some(5.),..Default::default()}).collect())
    }
    #[test]fn t7_test_report_is_a_valid_docx_with_table_chart_and_appendix(){
        let mut r=run(7,true,&[(0.5,0.51),(0.5,0.49),(0.5,0.5),(1.,1.01),(1.,1.01),(1.,1.01),(5.,4.975),(5.,5.025),(5.,5.15)]);
        r.1[8].excluded=true;r.1[8].exclude_reason="luchtbel".into();
        let b=test_report(&[r.clone()],Some("nr 1, 02-11-2026")).unwrap();
        let files=unzip(&b);let get=|n:&str|String::from_utf8(files.iter().find(|f|f.0==n).unwrap_or_else(||panic!("{n} ontbreekt")).1.clone()).unwrap();
        for f in &files{if f.0.ends_with(".xml") || f.0.ends_with(".rels"){well_formed(&String::from_utf8(f.1.clone()).unwrap()).unwrap_or_else(|e|panic!("{}: {e}",f.0));}}
        let doc=get("word/document.xml");
        assert!(doc.contains("Doseertest P_C01 — SIMULATIE"));assert!(doc.contains("Inleiding") && doc.contains("Conclusie en advies") && doc.contains("[In te vullen"));
        assert!(doc.contains("Bijlage: alle doses") && doc.contains("uitgesloten: luchtbel"));
        assert!(doc.contains("1 dosis/doses niet meegenomen: 1× luchtbel"),"uitsluiting genoemd");
        // Tables: setup, per weight (header + 3 weights + total), small doses, appendix (header + 9).
        assert_eq!(doc.matches("<w:tbl>").count(),4);
        let t1=doc.split("<w:tbl>").nth(2).unwrap();assert_eq!(t1.matches("<w:tr>").count(),5,"kop + 3 gewichten + totaal");
        let app=doc.split("<w:tbl>").nth(4).unwrap();assert_eq!(app.matches("<w:tr>").count(),10);
        // 5 g row: the excluded dose is not counted: 2/2 within. 1 g row: +1,00 %.
        assert!(t1.contains("+1,00 %") && t1.contains("2/2"),"{t1}");
        assert!(files.iter().any(|f|f.0=="word/charts/chart1.xml") && files.iter().any(|f|f.0=="word/charts/chart2.xml"));
        let hw=test_report(&[run(8,false,&[(1.,1.)])],None).unwrap();assert!(!String::from_utf8_lossy(&hw).contains("SIMULATIE"));
        // Several runs of one pump together; never simulation with hardware, never two pumps.
        let two=test_report(&[r.clone(),run(9,true,&[(1.,1.02)])],None).unwrap();assert!(String::from_utf8_lossy(&two).contains("2 testen"));
        assert!(test_report(&[r.clone(),run(8,false,&[(1.,1.)])],None).unwrap_err().contains("Simulatie- en hardware"));
        let mut other=run(10,true,&[(1.,1.)]);other.0.pump="P_C02".into();assert!(test_report(&[r,other],None).is_err());
        assert!(test_report(&[],None).is_err());
    }
}
