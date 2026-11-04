//! Pomptest: test plans (one weight, series, random with a fixed seed), statistics per requested weight,
//! recommended correction and the CSV export. Pure functions; running a test lives in `devices.rs`,
//! storage in `testdb.rs`, the Word report in `report.rs`.
use crate::model::{stats,Result};
use serde::{Deserialize,Serialize};

#[derive(Clone,Copy,Debug,PartialEq,Eq,Serialize,Deserialize)]
pub enum Mode{Enkel,Reeks,Willekeurig}
impl Mode{
    pub fn key(self)->&'static str{match self{Mode::Enkel=>"enkel",Mode::Reeks=>"reeks",Mode::Willekeurig=>"willekeurig"}}
    pub fn label(self)->&'static str{match self{Mode::Enkel=>"Eén gewicht",Mode::Reeks=>"Reeks",Mode::Willekeurig=>"Willekeurig"}}
    pub fn from_key(k:&str)->Mode{match k{"reeks"=>Mode::Reeks,"willekeurig"=>Mode::Willekeurig,_=>Mode::Enkel}}
}

/// Everything that defines a test; stored as JSON with the run (seed included, so a random test repeats).
#[derive(Clone,Debug,PartialEq,Serialize,Deserialize)]
pub struct TestPlan{
    pub mode:Mode,
    /// Enkel: the first weight; Reeks: all weights.
    pub weights:Vec<f64>,pub repeats:usize,
    /// Reeks: false = ascending, each weight `repeats` times in a row; true = shuffled with `seed`.
    pub shuffle:bool,
    /// Willekeurig: `n` doses uniform between `min` and `max` g, rounded to 0,001 g.
    pub n:usize,pub min:f64,pub max:f64,pub seed:u64,
    /// Tolerance ±% (default 1), wait before the stable weighing (s), tare before every dose, pause after every dose.
    pub tolerance_pct:f64,pub settle_s:f64,pub tare_each:bool,pub pause_each:bool,
}
impl Default for TestPlan{
    fn default()->Self{Self{mode:Mode::Reeks,weights:vec![0.5,1.,2.,3.,4.,5.,10.,15.,20.],repeats:3,shuffle:false,n:20,min:0.5,max:5.,seed:1,
        tolerance_pct:1.,settle_s:2.,tare_each:true,pause_each:false}}
}
pub const MAX_DOSES:usize=1000;

/// Small deterministic generator (xorshift64*): the same seed always gives the same numbers.
pub struct Rng(u64);
impl Rng{
    pub fn new(seed:u64)->Self{Self((seed^0x9E37_79B9_7F4A_7C15).max(1))}
    pub fn next_u64(&mut self)->u64{let mut x=self.0;x^=x>>12;x^=x<<25;x^=x>>27;self.0=x;x.wrapping_mul(0x2545_F491_4F6C_DD1D)}
    /// Uniform in [0, 1).
    pub fn unit(&mut self)->f64{(self.next_u64()>>11) as f64/(1u64<<53) as f64}
    pub fn below(&mut self,n:usize)->usize{(self.unit()*n as f64) as usize%n.max(1)}
}

impl TestPlan{
    pub fn validate(&self)->Result<()>{
        if !self.tolerance_pct.is_finite() || !(0.01..=50.).contains(&self.tolerance_pct){return Err("Tolerantie 0,01 tot 50 %".into());}
        if !self.settle_s.is_finite() || !(0. ..=60.).contains(&self.settle_s){return Err("Wachttijd 0 tot 60 s".into());}
        match self.mode{
            Mode::Enkel|Mode::Reeks=>{
                if self.weights.is_empty(){return Err("Vul minimaal één gewicht in".into());}
                if let Some(w)=self.weights.iter().find(|w|!w.is_finite() || **w<=0. || **w>10000.){return Err(format!("Ongeldig gewicht {w} (0 tot 10 000 g)"));}
                if !(1..=MAX_DOSES).contains(&self.repeats){return Err(format!("Herhalingen 1 tot {MAX_DOSES}"));}
            }
            Mode::Willekeurig=>{
                if !(1..=MAX_DOSES).contains(&self.n){return Err(format!("Aantal doses 1 tot {MAX_DOSES}"));}
                if !self.min.is_finite() || !self.max.is_finite() || self.min<=0. || self.max<self.min || self.max>10000.{return Err("Minimum en maximum: 0 < min ≤ max ≤ 10 000 g".into());}
            }
        }
        Ok(())
    }
    /// The requested weights in dosing order.
    pub fn doses(&self)->Result<Vec<f64>>{
        self.validate()?;
        let mut rng=Rng::new(self.seed);
        let out:Vec<f64>=match self.mode{
            Mode::Enkel=>vec![self.weights[0];self.repeats],
            Mode::Reeks=>{
                let mut w=self.weights.clone();w.sort_by(f64::total_cmp);
                let mut v:Vec<f64>=w.iter().flat_map(|g|std::iter::repeat_n(*g,self.repeats)).collect();
                // Fisher–Yates with the seeded generator.
                if self.shuffle{for i in (1..v.len()).rev(){let j=rng.below(i+1);v.swap(i,j);}}
                v
            }
            Mode::Willekeurig=>(0..self.n).map(|_|{
                let g=self.min+rng.unit()*(self.max-self.min);
                ((g*1000.).round()/1000.).clamp((self.min*1000.).ceil()/1000.,(self.max*1000.).floor()/1000.).max(0.001)}).collect(),
        };
        if out.len()>MAX_DOSES{return Err(format!("Maximaal {MAX_DOSES} doses per test"));}
        Ok(out)
    }
}

/// One dose as stored (enough for statistics, table, report and CSV).
#[derive(Clone,Debug,Default,PartialEq,Serialize,Deserialize)]
pub struct DoseRow{
    pub id:i64,pub run_id:i64,pub nr:i64,pub requested_g:f64,pub dosed_g:Option<f64>,pub correction_pct:f64,pub target_g:Option<f64>,
    pub steps:Option<i64>,pub settings:String,pub duration_s:Option<f64>,pub status:String,pub error:String,pub excluded:bool,pub exclude_reason:String,pub time_ms:i64,
}
impl DoseRow{
    pub fn err_g(&self)->Option<f64>{self.dosed_g.map(|d|d-self.requested_g)}
    pub fn err_pct(&self)->Option<f64>{self.dosed_g.map(|d|100.*(d-self.requested_g)/self.requested_g)}
    /// Measured and not excluded: counts in the statistics.
    pub fn counts(&self)->bool{self.dosed_g.is_some() && !self.excluded}
    pub fn within(&self,tol:f64)->Option<bool>{self.err_pct().map(|e|e.abs()<=tol+1e-9)}
}

/// Result per requested weight (or weight class for random tests).
#[derive(Clone,Debug,PartialEq)]
pub struct WeightStats{
    pub label:String,pub requested:f64,
    /// Doses counted / excluded / failed (not measured).
    pub n:usize,pub excluded:usize,pub failed:usize,
    pub mean_g:f64,pub mean_err_g:f64,pub mean_err_pct:f64,pub sd_pct:f64,pub sd_g:f64,pub min_pct:f64,pub max_pct:f64,
    pub within:usize,pub within_pct:f64,
    /// Dosing speed: dosed grams per minute of dose time (None without durations).
    pub g_per_min:Option<f64>,
}

/// Statistics per requested weight; random tests with more than 12 distinct weights in five classes.
/// Excluded doses never count but are reported; nothing is removed automatically.
pub fn summary(doses:&[DoseRow],tol:f64)->Vec<WeightStats>{
    let mut weights:Vec<f64>=doses.iter().map(|d|d.requested_g).collect();weights.sort_by(f64::total_cmp);weights.dedup();
    let make=|label:String,set:Vec<&DoseRow>|->WeightStats{
        let used:Vec<&DoseRow>=set.iter().copied().filter(|d|d.counts()).collect();
        let p:Vec<f64>=used.iter().filter_map(|d|d.err_pct()).collect();let g:Vec<f64>=used.iter().filter_map(|d|d.err_g()).collect();
        let m:Vec<f64>=used.iter().filter_map(|d|d.dosed_g).collect();
        let (mp,sp)=stats(&p);let (mg,sg)=stats(&g);
        let within=used.iter().filter(|d|d.within(tol)==Some(true)).count();
        let dur:f64=used.iter().filter_map(|d|d.duration_s).sum();
        WeightStats{label,requested:stats(&set.iter().map(|d|d.requested_g).collect::<Vec<_>>()).0,n:used.len(),
            excluded:set.iter().filter(|d|d.excluded).count(),failed:set.iter().filter(|d|d.dosed_g.is_none()).count(),
            mean_g:stats(&m).0,mean_err_g:mg,mean_err_pct:mp,sd_pct:sp,sd_g:sg,
            min_pct:p.iter().cloned().fold(f64::INFINITY,f64::min),max_pct:p.iter().cloned().fold(f64::NEG_INFINITY,f64::max),
            within,within_pct:if used.is_empty(){0.}else{100.*within as f64/used.len() as f64},
            g_per_min:if dur>0.{Some(m.iter().sum::<f64>()/dur*60.)}else{None}}
    };
    if weights.len()<=12{
        return weights.iter().map(|w|make(format!("{} g",nl(*w,3)),doses.iter().filter(|d|d.requested_g==*w).collect())).collect();
    }
    let (lo,hi)=(weights[0],*weights.last().unwrap());let step=(hi-lo)/5.;
    (0..5).filter_map(|i|{
        let a=lo+step*i as f64;let b=if i==4{hi}else{lo+step*(i+1) as f64};
        let set:Vec<&DoseRow>=doses.iter().filter(|d|d.requested_g>=a && (d.requested_g<b || (i==4 && d.requested_g<=b))).collect();
        if set.is_empty(){None}else{Some(make(format!("{} – {} g",nl(a,3),nl(b,3)),set))}
    }).collect()
}
/// Totals over all counted doses.
pub fn overall(doses:&[DoseRow],tol:f64)->Option<WeightStats>{
    let all:Vec<DoseRow>=doses.iter().map(|d|DoseRow{requested_g:1.,..d.clone()}).collect();
    // Same statistics with one group; errors are taken from the real requested weights.
    let used:Vec<&DoseRow>=doses.iter().filter(|d|d.counts()).collect();
    if used.is_empty(){return None;}
    let mut s=summary(&all,tol).into_iter().next()?;
    let p:Vec<f64>=used.iter().filter_map(|d|d.err_pct()).collect();let g:Vec<f64>=used.iter().filter_map(|d|d.err_g()).collect();
    let (mp,sp)=stats(&p);let (mg,sg)=stats(&g);
    s.label="Totaal".into();s.requested=stats(&used.iter().map(|d|d.requested_g).collect::<Vec<_>>()).0;s.mean_g=stats(&used.iter().filter_map(|d|d.dosed_g).collect::<Vec<_>>()).0;
    s.mean_err_pct=mp;s.sd_pct=sp;s.mean_err_g=mg;s.sd_g=sg;s.min_pct=p.iter().cloned().fold(f64::INFINITY,f64::min);s.max_pct=p.iter().cloned().fold(f64::NEG_INFINITY,f64::max);
    s.within=used.iter().filter(|d|d.within(tol)==Some(true)).count();s.within_pct=100.*s.within as f64/used.len() as f64;
    Some(s)
}

/// Recommended correction for the next doses (U1). Each counted dose i was dosed with correction c_i and came
/// out e_i % off; the correction that would have hit the requested weight is c_i' = ((1+c_i/100)/(1+e_i/100) − 1)·100.
/// Proposal = mean c_i' of the last `last` counted doses, rounded to 0,1 % and within −20…+20 %. It also holds
/// when the correction was changed during the test. Returns the proposal and the first/last dose number used.
/// Only a proposal: the operator adopts it with a click. Counted doses that weighed 0 g or less (empty
/// stroke, scale glitch) carry no information about the correction: they are skipped and their number is
/// returned as the fourth value so the UI can say so.
pub fn recommended_correction(doses:&[DoseRow],last:usize)->Option<(f64,i64,i64,usize)>{
    let counted:Vec<&DoseRow>=doses.iter().filter(|d|d.counts() && d.dosed_g.is_some()).collect();
    let skipped=counted.iter().filter(|d|d.dosed_g.is_some_and(|g|g<=0.)).count();
    let used:Vec<(i64,f64)>=counted.iter().filter(|d|d.dosed_g.is_some_and(|g|g>0.)).filter_map(|d|d.err_pct().map(|e|(d.nr,((1.+d.correction_pct/100.)/(1.+e/100.)-1.)*100.))).collect();
    if used.is_empty() || last==0{return None;}
    let tail=&used[used.len().saturating_sub(last)..];
    let mean=tail.iter().map(|x|x.1).sum::<f64>()/tail.len() as f64;
    if !mean.is_finite(){return None;}
    Some((round01(mean.clamp(-20.,20.)),tail[0].0,tail[tail.len()-1].0,skipped))
}
/// Round to 0,1 % (no -0,0).
pub fn round01(v:f64)->f64{let r=(v*10.).round()/10.;if r==0.{0.}else{r}}
/// Correction from the UI: −20…+20 % in 0,1 % steps.
pub fn check_correction(c:f64)->Result<f64>{
    if !c.is_finite() || !(-20. ..=20.).contains(&c){return Err("Correctie −20 tot +20 %".into());}
    Ok(round01(c))
}

/// Drop size indication (ceramic): spread against requested weight for the small doses, and the smallest
/// tested weight whose spread (SD %) stays within the tolerance. An indication, no claim.
pub fn smallest_within(stats:&[WeightStats],tol:f64)->Option<f64>{
    stats.iter().filter(|s|s.n>=2 && s.sd_pct<=tol).map(|s|s.requested).fold(None,|a:Option<f64>,b|Some(a.map_or(b,|a|a.min(b))))
}

pub fn nl(v:f64,d:usize)->String{format!("{v:.d$}").replace('.',",")}
pub fn nls(v:f64,d:usize)->String{format!("{v:+.d$}").replace('.',",")}

/// CSV for Excel (Dutch): semicolon separated, decimal comma, UTF-8 with BOM. Doses, then the summary.
pub fn csv(title:&str,doses:&[DoseRow],tol:f64)->Vec<u8>{
    let q=|s:&str|if s.contains([';','"','\n']){format!("\"{}\"",s.replace('"',"\"\""))}else{s.to_string()};
    let o=|v:Option<f64>,d:usize|v.map(|x|nl(x,d)).unwrap_or_default();
    let mut out=String::from("\u{feff}");
    out+=&format!("{}\n",q(title));
    out+="Nr;Gevraagd (g);Gedoseerd (g);Afwijking (g);Afwijking (%);Binnen tolerantie;Correctie (%);Doel (g);Stappen;Duur (s);Status;Fout;Uitgesloten;Reden;Pompinstellingen\n";
    for d in doses{
        out+=&format!("{};{};{};{};{};{};{};{};{};{};{};{};{};{};{}\n",d.nr,nl(d.requested_g,4),o(d.dosed_g,4),o(d.err_g(),4),o(d.err_pct(),3),
            match d.within(tol){Some(true)=>"ja",Some(false)=>"nee",None=>""},nl(d.correction_pct,1),o(d.target_g,4),d.steps.map(|s|s.to_string()).unwrap_or_default(),
            o(d.duration_s,1),q(&d.status),q(&d.error),if d.excluded{"ja"}else{"nee"},q(&d.exclude_reason),q(&d.settings));
    }
    out+=&format!("\nSamenvatting (tolerantie ±{} %)\n",nl(tol,2));
    out+="Gevraagd;n;Uitgesloten;Niet gemeten;Gem. gedoseerd (g);Gem. afwijking (g);Gem. afwijking (%);Spreiding (%);Kleinste afw. (%);Grootste afw. (%);Binnen tolerantie;Binnen (%);g/min\n";
    for s in summary(doses,tol).iter().chain(overall(doses,tol).iter()){
        let f=|v:f64,d:usize|if v.is_finite(){nl(v,d)}else{String::new()};
        out+=&format!("{};{};{};{};{};{};{};{};{};{};{}/{};{};{}\n",q(&s.label),s.n,s.excluded,s.failed,f(s.mean_g,4),f(s.mean_err_g,4),f(s.mean_err_pct,3),f(s.sd_pct,3),
            f(s.min_pct,3),f(s.max_pct,3),s.within,s.n,f(s.within_pct,1),s.g_per_min.map(|g|nl(g,2)).unwrap_or_default());
    }
    out.into_bytes()
}

#[cfg(test)] mod tests{
    use super::*;
    fn d(nr:i64,req:f64,dosed:f64)->DoseRow{DoseRow{nr,requested_g:req,dosed_g:Some(dosed),status:"ok".into(),duration_s:Some(6.),..Default::default()}}
    #[test]fn t1_statistics_with_fixed_numbers(){
        // 0,5 g: +2 %, −2 %, 0 %; 1 g: +1 %, +1 %, +1 %; 5 g: −0,5 %, +0,5 %, +3 % (last one 3 % off).
        let doses=vec![d(1,0.5,0.51),d(2,0.5,0.49),d(3,0.5,0.5),d(4,1.,1.01),d(5,1.,1.01),d(6,1.,1.01),d(7,5.,4.975),d(8,5.,5.025),d(9,5.,5.15)];
        let s=summary(&doses,1.);assert_eq!(s.len(),3);
        let a=&s[0];assert_eq!((a.label.as_str(),a.n,a.within),("0,500 g",3,1));assert!(a.mean_err_pct.abs()<1e-9);assert!((a.sd_pct-2.).abs()<1e-9);
        assert!((a.min_pct+2.).abs()<1e-9 && (a.max_pct-2.).abs()<1e-9);assert!((a.within_pct-100./3.).abs()<1e-9);
        let b=&s[1];assert!((b.mean_err_pct-1.).abs()<1e-9 && b.sd_pct.abs()<1e-9);assert_eq!(b.within,3,"precies 1 % telt als binnen");assert!((b.mean_err_g-0.01).abs()<1e-12);
        let c=&s[2];assert!((c.mean_err_pct-1.).abs()<1e-9);
        // sd of (−0,5, 0,5, 3): mean 1, deviations −1,5 −0,5 2 → var (2,25+0,25+4)/2 = 3,25.
        assert!((c.sd_pct-3.25f64.sqrt()).abs()<1e-9,"{}",c.sd_pct);assert_eq!(c.within,2);
        // 3 doses of 5 g in 18 s: 15,15 g / 18 s × 60 = 50,5 g/min.
        assert!((c.g_per_min.unwrap()-50.5).abs()<1e-9);
        let t=overall(&doses,1.).unwrap();assert_eq!((t.n,t.within),(9,6));
        // Excluding the 3 % dose: it no longer counts, but it is reported.
        let mut ex=doses.clone();ex[8].excluded=true;ex[8].exclude_reason="luchtbel".into();
        let c2=&summary(&ex,1.)[2];assert_eq!((c2.n,c2.excluded,c2.within),(2,1,2));assert!(c2.mean_err_pct.abs()<1e-9);
        // A failed dose (not measured) is counted as failed, never as a result.
        let mut f=doses.clone();f[0].dosed_g=None;f[0].status="fout".into();assert_eq!((summary(&f,1.)[0].n,summary(&f,1.)[0].failed),(2,1));
    }
    #[test]fn t2_random_plan_repeats_with_the_same_seed(){
        let p=TestPlan{mode:Mode::Willekeurig,n:50,min:0.5,max:5.,seed:42,..Default::default()};
        let a=p.doses().unwrap();assert_eq!(a.len(),50);assert_eq!(a,p.doses().unwrap(),"zelfde zaad, zelfde gewichten");
        assert!(a.iter().all(|g|(0.5..=5.).contains(g) && ((g*1000.).round()-g*1000.).abs()<1e-6));
        assert_ne!(a,TestPlan{seed:43,..p.clone()}.doses().unwrap());
        let r=TestPlan{mode:Mode::Reeks,weights:vec![5.,0.5,1.],repeats:3,shuffle:false,..Default::default()};
        assert_eq!(r.doses().unwrap(),vec![0.5,0.5,0.5,1.,1.,1.,5.,5.,5.]);
        let s=TestPlan{shuffle:true,seed:7,..r.clone()};let x=s.doses().unwrap();assert_eq!(x,s.doses().unwrap());
        let mut y=x.clone();y.sort_by(f64::total_cmp);assert_eq!(y,r.doses().unwrap(),"door elkaar: zelfde doses");
        assert_eq!(TestPlan{mode:Mode::Enkel,weights:vec![2.],repeats:4,..Default::default()}.doses().unwrap(),vec![2.;4]);
        for bad in [TestPlan{weights:vec![],..Default::default()},TestPlan{weights:vec![-1.],..Default::default()},TestPlan{repeats:0,..Default::default()},
            TestPlan{mode:Mode::Willekeurig,min:5.,max:1.,..Default::default()},TestPlan{tolerance_pct:0.,..Default::default()},TestPlan{repeats:2000,..Default::default()}]{assert!(bad.doses().is_err());}
    }
    fn dc(nr:i64,dosed:f64,corr:f64)->DoseRow{DoseRow{correction_pct:corr,..d(nr,1.,dosed)}}
    #[test]fn t3_recommended_correction_and_steps(){
        // Constant correction 0: c' = 1/(1+e) − 1 ≈ −e.
        let doses=vec![d(1,1.,1.04),d(2,1.,1.05),d(3,1.,1.03)];
        let (r,a,b,_)=recommended_correction(&doses,3).unwrap();assert_eq!((r,a,b),(-3.8,1,3));
        assert_eq!(recommended_correction(&doses,2).map(|x|x.0),Some(-3.8));
        let mut ex=doses.clone();ex[1].excluded=true;assert_eq!(recommended_correction(&ex,3).map(|x|(x.0,x.1,x.2)),Some((-3.4,1,3)),"uitgesloten telt niet");
        assert_eq!(recommended_correction(&[],3),None);assert_eq!(recommended_correction(&[d(1,1.,1.5)],3).map(|x|x.0),Some(-20.),"begrensd");
        // Critic sequence: pump doses +4 %. Doses 1–3 at 0 % (+4 %), correction −4 % adopted, dose 4 at −4 % (−0,16 %).
        let seq=vec![dc(1,1.04,0.),dc(2,1.04,0.),dc(3,1.04,0.),dc(4,0.9984,-4.)];
        // Binding formula: every dose says "−3,85 % would have hit 1 g" (1/1,04 − 1; 0,96/0,9984 − 1), so −3,8 %,
        // not the −7,0 % of the old rule. Check: 1,04 × (1 − 0,0385) = 1,000 g.
        for n in [3,5]{let (r,_,_,_)=recommended_correction(&seq,n).unwrap();assert_eq!(r,-3.8,"laatste {n}");assert!((1.04*(1.+r/100.)-1.).abs()<0.002);}
        assert_eq!(recommended_correction(&seq,5).map(|x|(x.1,x.2)),Some((1,4)));
        // Only the dose with −4 %: still about −3,8 % (1,04 × 0,96 = 0,9984 → needed (0,96/0,9984 − 1) = −3,85 %).
        assert_eq!(recommended_correction(&seq[3..],1).map(|x|x.0),Some(-3.8));
        // Constant correction −4 % and still +0,26 %: 0,96 / 1,0026 − 1 = −4,25 % → −4,2 %.
        assert_eq!(recommended_correction(&[dc(1,1.0026,-4.),dc(2,1.0026,-4.)],5).map(|x|x.0),Some(-4.2));
        // Carry-over iteration 3: a dose of 0 g or less is skipped (and counted), not a reason for None or −20 %.
        let mut z=seq.clone();z[1].dosed_g=Some(0.);z[2].dosed_g=Some(-0.01);
        let (r,a,b,sk)=recommended_correction(&z,5).unwrap();assert_eq!((r,sk),(-3.8,2));assert_eq!((a,b),(1,4));
        let only:Vec<DoseRow>=z[1..3].to_vec();assert_eq!(recommended_correction(&only,5),None,"alleen 0 g: geen voorstel");
        assert_eq!(check_correction(0.1+0.2),Ok(0.3));assert!(check_correction(20.1).is_err());assert_eq!(check_correction(-0.04),Ok(0.));
    }
    #[test]fn csv_is_dutch_and_complete(){
        let mut doses=vec![d(1,0.5,0.51),d(2,0.5,0.49)];doses[1].excluded=true;doses[1].exclude_reason="druppel; aan rand".into();
        let b=csv("Test P_C01",&doses,1.);assert!(b.starts_with(&[0xEF,0xBB,0xBF]));
        let t=String::from_utf8(b[3..].to_vec()).unwrap();
        assert!(t.contains("1;0,5000;0,5100;0,0100;2,000;nee;0,0;"),"{t}");assert!(t.contains("\"druppel; aan rand\""));
        assert!(t.contains("0,500 g;1;1;0;0,5100"),"{t}");
    }
}
