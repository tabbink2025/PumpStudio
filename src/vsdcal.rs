//! VSD pump calibration as Camelot CVSDDispensePosition, kept by Pump Studio itself
//! (`machine/calibration/<pump>.json`).
//! Phase 1: weight against shaft position within one stroke (cosine-spaced 0..210°), averaged runs,
//! outliers repaired (Hampel on segment slopes, PCHIP from clean points). Max = weight per stroke.
//! Phase 2: cumulative weight after whole rotations (1,2,3,5,7,10,13,16), averaged runs.
//! Dosing: weight(pos) = Phase-2 weight of whole rotations + Phase-1 weight of the part-stroke; the
//! target is found from the pump's actual position (phase-aware, unlike Camelot which assumes phase 0).
use crate::model::Result;
use serde::{Deserialize,Serialize};
use std::path::{Path,PathBuf};

#[derive(Clone,Debug,Serialize,Deserialize,PartialEq)]
pub struct CalSettings{pub phase1_samples:usize,pub phase1_runs:usize,pub phase1_max_deg:f64,pub phase2_runs:usize,pub stroke_samples:Vec<u32>,pub settle_s:f64,pub phase2:bool}
impl Default for CalSettings{
    // Camelot defaults (VSDDispensePositionModel.cpp).
    fn default()->Self{Self{phase1_samples:41,phase1_runs:2,phase1_max_deg:210.,phase2_runs:2,stroke_samples:vec![1,2,3,5,7,10,13,16],settle_s:2.,phase2:true}}
}
impl CalSettings{
    pub fn validate(&self)->Result<()>{
        if !(3..=200).contains(&self.phase1_samples) || !(1..=10).contains(&self.phase1_runs) || !(1..=10).contains(&self.phase2_runs){return Err("Meetpunten 3–200, runs 1–10".into());}
        if !(30. ..=360.).contains(&self.phase1_max_deg) || !(0. ..=60.).contains(&self.settle_s){return Err("Fase 1 tot 30–360°, wachttijd 0–60 s".into());}
        if self.phase2 && (self.stroke_samples.is_empty() || self.stroke_samples.windows(2).any(|w|w[1]<=w[0]) || self.stroke_samples[0]==0 || *self.stroke_samples.last().unwrap()>200){
            return Err("Omwentelingen fase 2: oplopend, 1 tot 200".into());}
        Ok(())
    }
}

#[derive(Clone,Debug,Default,Serialize,Deserialize,PartialEq)]
pub struct PdPoint{pub pos:f64,pub g:f64,#[serde(default)] pub repaired:bool}
#[derive(Clone,Debug,Default,Serialize,Deserialize,PartialEq)]
pub struct StrokePoint{pub rot:u32,pub g:f64}

#[derive(Clone,Debug,Default,Serialize,Deserialize)]
pub struct VsdCalibration{
    pub pump:String,pub head:String,pub node:u16,pub drive:u8,pub date_ms:u128,pub simulation:bool,
    pub settings:CalSettings,
    /// Phase 1 averaged and repaired; position in revolutions (< 1).
    pub pd:Vec<PdPoint>,
    /// Phase 1 raw runs (grams per sample index).
    pub pd_runs:Vec<Vec<f64>>,
    /// Phase 2 averaged; cumulative grams after `rot` whole rotations.
    pub stroke:Vec<StrokePoint>,pub stroke_runs:Vec<Vec<f64>>,
    pub g_per_stroke:f64,
    /// Steps per revolution of the pump at calibration time (positions in the database are in steps).
    #[serde(default="default_spr")] pub spr:i64,
}

/// Camelot spacing: p_i = max/2·(1−cos(πi/(N−1))) revolutions, denser at both ends.
pub fn cosine_positions(n:usize,max_deg:f64)->Vec<f64>{
    let max=max_deg/360.;(0..n).map(|i|0.5*max*(1.-(std::f64::consts::PI*i as f64/(n-1) as f64).cos())).collect()
}

// ---------- PCHIP (Fritsch–Carlson) ----------
fn pchip_slopes(x:&[f64],y:&[f64])->Vec<f64>{
    let n=x.len();let h:Vec<f64>=(0..n-1).map(|i|x[i+1]-x[i]).collect();let d:Vec<f64>=(0..n-1).map(|i|(y[i+1]-y[i])/h[i]).collect();
    let mut m=vec![0.;n];
    if n==2{m[0]=d[0];m[1]=d[0];return m;}
    for k in 1..n-1{
        if d[k-1]*d[k]>0.{let w1=2.*h[k]+h[k-1];let w2=h[k]+2.*h[k-1];m[k]=(w1+w2)/(w1/d[k-1]+w2/d[k]);}
    }
    let end=|h0:f64,h1:f64,d0:f64,d1:f64|{let s=((2.*h0+h1)*d0-h0*d1)/(h0+h1);if s.signum()!=d0.signum(){0.}else if d0.signum()!=d1.signum() && s.abs()>3.*d0.abs(){3.*d0}else{s}};
    m[0]=end(h[0],h[1],d[0],d[1]);m[n-1]=end(h[n-2],h[n-3],d[n-2],d[n-3]);m
}
pub fn pchip(x:&[f64],y:&[f64],t:f64)->f64{
    let n=x.len();if n==0{return 0.;}if n==1{return y[0];}
    if t<=x[0]{return y[0];}if t>=x[n-1]{return y[n-1];}
    let m=pchip_slopes(x,y);let k=x.partition_point(|v|*v<=t).saturating_sub(1).min(n-2);
    let h=x[k+1]-x[k];let s=(t-x[k])/h;
    let h00=2.*s*s*s-3.*s*s+1.;let h10=s*s*s-2.*s*s+s;let h01=-2.*s*s*s+3.*s*s;let h11=s*s*s-s*s;
    h00*y[k]+h10*h*m[k]+h01*y[k+1]+h11*h*m[k+1]
}

/// Hampel filter on segment slopes (median ± 3·1.4826·MAD); an interior point with an outlying slope
/// on both sides is replaced by PCHIP through the clean points. Up to three passes, like Camelot.
pub fn repair(x:&[f64],y:&[f64])->(Vec<f64>,Vec<bool>){
    let n=x.len();let mut y=y.to_vec();let mut fixed=vec![false;n];
    if n<5{return (y,fixed);}
    let median=|v:&mut Vec<f64>|{v.sort_by(f64::total_cmp);let k=v.len();if k%2==1{v[k/2]}else{(v[k/2-1]+v[k/2])/2.}};
    for _ in 0..3{
        let s:Vec<f64>=(0..n-1).map(|i|(y[i+1]-y[i])/(x[i+1]-x[i]).max(1e-12)).collect();
        let med=median(&mut s.clone());let mad=median(&mut s.iter().map(|v|(v-med).abs()).collect());
        let lim=3.*1.4826*mad.max(1e-9);let bad:Vec<bool>=s.iter().map(|v|(v-med).abs()>lim).collect();
        let flag:Vec<usize>=(1..n-1).filter(|j|bad[j-1]&&bad[*j]).collect();
        if flag.is_empty(){break;}
        let (cx,cy):(Vec<f64>,Vec<f64>)=(0..n).filter(|i|!flag.contains(i)).map(|i|(x[i],y[i])).unzip();
        for j in flag{y[j]=pchip(&cx,&cy,x[j]);fixed[j]=true;}
    }
    (y,fixed)
}

impl VsdCalibration{
    /// Build the averaged, repaired tables from raw runs.
    pub fn finish(&mut self){
        let n=self.settings.phase1_samples;let pos=cosine_positions(n,self.settings.phase1_max_deg);
        let avg:Vec<f64>=(0..n).map(|i|{let v:Vec<f64>=self.pd_runs.iter().filter_map(|r|r.get(i).copied()).collect();if v.is_empty(){0.}else{v.iter().sum::<f64>()/v.len() as f64}}).collect();
        // Curve starts at zero (first sample is weighed right after tare); negatives clamped like Camelot.
        let zero=avg.first().copied().unwrap_or(0.);let avg:Vec<f64>=avg.iter().map(|g|(g-zero).max(0.)).collect();
        let (y,fixed)=repair(&pos,&avg);
        self.pd=pos.iter().zip(&y).zip(&fixed).map(|((p,g),f)|PdPoint{pos:*p,g:*g,repaired:*f}).collect();
        self.g_per_stroke=y.iter().cloned().fold(0.,f64::max);
        self.stroke=self.settings.stroke_samples.iter().enumerate().filter_map(|(k,rot)|{
            let v:Vec<f64>=self.stroke_runs.iter().filter_map(|r|r.get(k).copied()).collect();
            (!v.is_empty()).then(||StrokePoint{rot:*rot,g:v.iter().sum::<f64>()/v.len() as f64})}).collect();
    }
    /// Weight of a part-stroke position (0..1 rev) from Phase 1; the fill part after the last sample delivers nothing more.
    pub fn part(&self,frac:f64)->f64{
        let (x,y):(Vec<f64>,Vec<f64>)=self.pd.iter().map(|p|(p.pos,p.g)).unzip();
        if x.is_empty(){return frac.clamp(0.,0.5)*2.*self.g_per_stroke;}
        pchip(&x,&y,frac.clamp(0.,*x.last().unwrap())).max(0.)
    }
    /// Least-squares grams per rotation over Phase 2, leaving out the first point (inflated first stroke).
    fn steady_rate(&self)->f64{
        let pts:Vec<&StrokePoint>=self.stroke.iter().skip(1).collect();
        if pts.len()<2{return self.g_per_stroke;}
        let n=pts.len() as f64;let mx=pts.iter().map(|p|p.rot as f64).sum::<f64>()/n;let my=pts.iter().map(|p|p.g).sum::<f64>()/n;
        let sxx:f64=pts.iter().map(|p|(p.rot as f64-mx).powi(2)).sum();let sxy:f64=pts.iter().map(|p|(p.rot as f64-mx)*(p.g-my)).sum();
        if sxx>0.{sxy/sxx}else{self.g_per_stroke}
    }
    /// Cumulative weight of `rot` whole rotations (Phase 2; linear inside, from 0 below, steady rate above).
    pub fn rotations(&self,rot:f64)->f64{
        if self.stroke.len()<2{return rot*self.g_per_stroke;}
        let first=&self.stroke[0];let last=self.stroke.last().unwrap();
        if rot<=first.rot as f64{return first.g*rot/first.rot as f64;}
        if rot>=last.rot as f64{return last.g+(rot-last.rot as f64)*self.steady_rate();}
        let k=self.stroke.partition_point(|p|(p.rot as f64)<=rot)-1;let a=&self.stroke[k];let b=&self.stroke[k+1];
        a.g+(b.g-a.g)*(rot-a.rot as f64)/(b.rot-a.rot) as f64
    }
    /// Model cumulative grams at a shaft position measured in revolutions from a stroke start: whole
    /// strokes from Phase 2, inside a stroke that stroke's weight shaped by the Phase-1 curve, so the
    /// model is continuous and rising across stroke boundaries.
    pub fn cumulative(&self,pos:f64)->f64{
        let r=pos.floor();let a=self.rotations(r);let b=self.rotations(r+1.);
        let shape=if self.g_per_stroke>0.{(self.part(pos-r)/self.g_per_stroke).clamp(0.,1.)}else{0.};
        a+(b-a)*shape
    }
    /// First position ≥ start whose cumulative weight is `grams` more than at start (bisection).
    pub fn target(&self,start_rev:f64,grams:f64)->Result<f64>{
        if !(grams>0.) || !grams.is_finite(){return Err("Gewicht moet positief zijn".into());}
        if self.g_per_stroke<=0.{return Err("Kalibratie zonder opbrengst".into());}
        let base=self.cumulative(start_rev);let want=base+grams;
        let mut lo=start_rev;let mut hi=start_rev+1.;
        while self.cumulative(hi)<want{hi+= (hi-start_rev).max(1.);if hi-start_rev>5000.{return Err("Gewicht te groot voor kalibratie".into());}}
        for _ in 0..80{let mid=(lo+hi)/2.;if self.cumulative(mid)>=want{hi=mid;}else{lo=mid;}}
        Ok(hi)
    }
    pub fn path(dir:&Path,pump:&str)->PathBuf{dir.join(format!("{}.json",pump.replace(['/','\\',':'],"_")))}
    pub fn save(&self,dir:&Path)->Result<PathBuf>{
        std::fs::create_dir_all(dir).map_err(|e|e.to_string())?;let p=Self::path(dir,&self.pump);let tmp=p.with_extension("json.tmp");
        std::fs::write(&tmp,serde_json::to_vec_pretty(self).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
        std::fs::rename(&tmp,&p).map_err(|e|e.to_string())?;Ok(p)
    }
    pub fn load(dir:&Path,pump:&str)->Option<Self>{std::fs::read(Self::path(dir,pump)).ok().and_then(|b|serde_json::from_slice(&b).ok())}
}

#[cfg(test)] mod tests{
    use super::*;
    fn sim_cal()->VsdCalibration{
        // Diaphragm model 0.5 g/stroke delivered over the first half revolution, first stroke 10% high.
        let s=CalSettings::default();let pos=cosine_positions(s.phase1_samples,s.phase1_max_deg);
        let f=|p:f64|0.5*(2.*p).min(1.);
        let mut c=VsdCalibration{settings:s.clone(),..Default::default()};
        c.pd_runs=vec![pos.iter().map(|p|f(*p)).collect(),pos.iter().map(|p|f(*p)+0.002).collect()];
        c.stroke_runs=vec![s.stroke_samples.iter().map(|k|*k as f64*0.5+0.05).collect();2];
        c.finish();c
    }
    #[test]fn positions_and_tables(){
        let p=cosine_positions(41,210.);assert_eq!(p.len(),41);assert_eq!(p[0],0.);assert!((p[40]-210./360.).abs()<1e-12);assert!(p[1]-p[0]<p[21]-p[20]);
        let c=sim_cal();assert!((c.g_per_stroke-0.5).abs()<1e-9);assert_eq!(c.pd[0].g,0.);assert_eq!(c.stroke.len(),8);
        assert!((c.rotations(4.)-2.05).abs()<1e-9);assert!((c.rotations(20.)-10.05).abs()<1e-6);
    }
    #[test]fn outlier_is_repaired(){
        let x:Vec<f64>=(0..20).map(|i|i as f64/40.).collect();let mut y:Vec<f64>=x.iter().map(|v|v*1.).collect();y[9]+=0.2;
        let (r,f)=repair(&x,&y);assert!(f[9]);assert!((r[9]-x[9]).abs()<0.01);assert_eq!(f.iter().filter(|v|**v).count(),1);
    }
    #[test]fn dose_targets_are_phase_aware(){
        let c=sim_cal();
        let t=c.target(0.,3.).unwrap();assert!((c.cumulative(t)-c.cumulative(0.)-3.).abs()<1e-6);
        // Starting in the fill part (0.6 rev): nothing more until the next stroke starts.
        let t2=c.target(0.6,0.1).unwrap();assert!(t2>1. && t2<1.2,"{t2}");
        let t3=c.target(0.,0.2).unwrap();assert!((t3-0.2/0.55*0.5).abs()<0.005,"{t3}");
        assert!(c.target(0.,0.).is_err());
        let dir=std::env::temp_dir().join(format!("vsdcal-{}",std::process::id()));let mut c2=c.clone();c2.pump="P_A01".into();
        c2.save(&dir).unwrap();assert_eq!(VsdCalibration::load(&dir,"P_A01").unwrap().stroke,c.stroke);
    }
}
fn default_spr()->i64{51200}
impl VsdCalibration{
    /// Steps per revolution used for positions in steps (51 200 when unknown).
    pub fn steps_per_rev(&self)->i64{if self.spr>0{self.spr}else{default_spr()}}
}
