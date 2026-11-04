use serde::{Deserialize, Serialize};
pub type Result<T> = std::result::Result<T, String>;
pub const MAX_POSITION: i64 = i32::MAX as i64;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub pump_type: String, pub steps_rev: i64, pub node: u32, pub motor: u8,
    pub speed: i64, pub accel: i64, pub decel: i64,
    pub channel: u32, pub bitrate: u32, pub port: String, pub baud: u32,
    pub require_feedback: bool, pub fluid: String, pub stroke_setting: String,
    pub window_s: f64, pub range_g: f64, pub timeout_s: f64, pub settle_s: f64,
    /// Highest controller counter we send; above ~10.7M steps the controller target wraps and the pump runs backward.
    #[serde(default="default_controller_limit")] pub controller_limit: i64,
    /// Off: connect pump only (moving, referencing, settings); every weighing action is blocked.
    #[serde(default="default_true")] pub use_scale: bool,
    /// Scale over network (Mettler MT-SICS on raw TCP, as Camelot's LH1/LH2 scales) instead of COM port.
    #[serde(default)] pub scale_tcp: bool,
    #[serde(default="default_scale_host")] pub scale_host: String,
    #[serde(default="default_scale_tcp_port")] pub scale_tcp_port: u16,
    /// Network scales of both heads at once (LH1 = scale_host, LH2 = scale_host2); measure_head 0/1 is weighed.
    #[serde(default)] pub scale_dual: bool,
    #[serde(default="default_scale_host2")] pub scale_host2: String,
    #[serde(default="default_scale_tcp_port")] pub scale_tcp_port2: u16,
    #[serde(default)] pub measure_head: u8,
}
fn default_true()->bool{true}
// Camelot TST_Guus_V4: Scale LH1 VSD 192.168.0.56:23, Scale LH2 VSD 192.168.0.55:23.
fn default_scale_host()->String{"192.168.0.56".into()}
fn default_scale_tcp_port()->u16{23}
fn default_scale_host2()->String{"192.168.0.55".into()}
pub const CONTROLLER_LIMIT_MAX: i64 = 10_500_000;
fn default_controller_limit()->i64{10_000_000}
impl Default for Config {
    fn default() -> Self { Self { pump_type:"INV-13".into(),steps_rev:25600,node:610,motor:2,
        speed:51200,accel:51200,decel:51200,channel:0,bitrate:250000,port:"COM9".into(),baud:9600,
        require_feedback:false,fluid:"water".into(),stroke_setting:String::new(),
        window_s:1.2,range_g:0.002,timeout_s:30.,settle_s:2.,controller_limit:default_controller_limit(),use_scale:true,
        scale_tcp:false,scale_host:default_scale_host(),scale_tcp_port:default_scale_tcp_port(),
        scale_dual:false,scale_host2:default_scale_host2(),scale_tcp_port2:default_scale_tcp_port(),measure_head:0 } }
}
impl Config {
    /// CAN bus; changing it needs a new connection. Pump (node/motor) and scale (on/off, port, baud) can change live.
    pub fn same_connection(&self, other:&Config) -> bool {
        self.channel==other.channel && self.bitrate==other.bitrate
    }
    pub fn same_scale(&self, other:&Config) -> bool {
        self.use_scale==other.use_scale && self.port.trim()==other.port.trim() && self.baud==other.baud
            && self.scale_tcp==other.scale_tcp && self.scale_host.trim()==other.scale_host.trim() && self.scale_tcp_port==other.scale_tcp_port
            && self.scale_dual==other.scale_dual && self.scale_host2.trim()==other.scale_host2.trim() && self.scale_tcp_port2==other.scale_tcp_port2
    }
    /// Different physical pump: node, motor, type or steps per revolution.
    pub fn same_pump(&self, other:&Config) -> bool {
        self.node==other.node && self.motor==other.motor && self.pump_type==other.pump_type && self.steps_rev==other.steps_rev
    }
    pub fn validate(&self) -> Result<()> {
        if !["INV-13","INV-36"].contains(&self.pump_type.as_str()) || self.node>2047 || self.node==0 || !(1..=8).contains(&self.motor) {
            return Err("Controleer pomptype, CAN-node en motor".into());
        }
        if [self.steps_rev,self.speed,self.accel,self.decel].iter().any(|v|*v<=0 || *v>MAX_POSITION) {
            return Err("Motorinstellingen moeten positief en binnen 32-bit bereik zijn".into());
        }
        let scale_missing=if self.scale_tcp{self.scale_host.trim().is_empty() || self.scale_tcp_port==0 || (self.scale_dual && (self.scale_host2.trim().is_empty() || self.scale_tcp_port2==0))}else{self.port.trim().is_empty() || self.baud==0};
        if self.measure_head>1 || (self.measure_head==1 && !(self.scale_tcp && self.scale_dual)){return Err("Meetkop LH2 alleen met beide netwerkweegschalen".into());}
        if (self.use_scale && scale_missing) || self.bitrate==0 || self.fluid.trim().is_empty() {
            return Err("Poort, baudrate, bitrate en vloeistof ontbreken".into());
        }
        if [self.window_s,self.range_g,self.timeout_s,self.settle_s].iter().any(|x|!x.is_finite()) || self.window_s<=0. || self.range_g<=0. || self.settle_s<0. || self.settle_s>300. || self.timeout_s<=self.window_s.max(self.settle_s) {
            return Err("Ongeldige stabiliteitsinstellingen".into());
        }
        if !(100_000..=CONTROLLER_LIMIT_MAX).contains(&self.controller_limit) || self.controller_limit<self.steps_rev {
            return Err(format!("Controllergrens: 100.000 tot {CONTROLLER_LIMIT_MAX} stappen en minimaal één omwenteling"));
        } Ok(())
    }
}

pub fn corrected_mass(grams:f64, percent:f64) -> Result<f64> {
    if !grams.is_finite() || grams<=0. || !percent.is_finite() || !(-20. ..=20.).contains(&percent) {
        return Err("Massa moet positief zijn; correctie tussen -20 en +20%".into());
    }
    let value=grams*(1.+percent/100.);
    if !value.is_finite() { return Err("Doelmassa te groot".into()); } Ok(value)
}

pub fn angle_targets(steps:i64, interval:f64) -> Result<Vec<(f64,i64)>> {
    if steps<=0 || steps>MAX_POSITION || !interval.is_finite() || interval<=0. || interval>360. || (360./interval).ceil()>10000. {
        return Err("Ongeldig hoekinterval".into());
    }
    let mut out=Vec::new();let mut prev=0;
    for i in 1..=(360./interval).ceil() as usize {
        let angle=(i as f64*interval).min(360.);
        let target=(steps as f64*angle/360.).round() as i64;
        if target<=prev {return Err("Hoekinterval geeft dubbele posities".into());}
        out.push((target as f64*360./steps as f64,target));prev=target;
    } Ok(out)
}

/// Characterization grid: fixed angle or fixed step count per point; last point always lands on full revolution.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
pub enum Interval { Degrees(f64), Steps(i64) }
impl Interval {
    pub fn targets(&self, steps:i64) -> Result<Vec<(f64,i64)>> {
        match *self { Interval::Degrees(d)=>angle_targets(steps,d), Interval::Steps(s)=>step_targets(steps,s) }
    }
}
pub fn step_targets(steps:i64, step:i64) -> Result<Vec<(f64,i64)>> {
    if steps<=0 || steps>MAX_POSITION || step<=0 || step>steps || (steps+step-1)/step>10000 {
        return Err("Ongeldige stapgrootte".into());
    }
    let mut out=Vec::new();let mut target=0;
    while target<steps {target=(target+step).min(steps);out.push((target as f64*360./steps as f64,target));}
    Ok(out)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Curve {
    pub steps_rev:i64, pub origin:i64, pub steps:Vec<i64>, pub masses:Vec<f64>,
    pub raw_mean:Vec<f64>, pub sd:Vec<f64>, pub interval_sd:Vec<f64>,pub runs:Vec<Vec<f64>>,pub correction_g:f64,
}
impl Curve {
    pub fn fit(steps_rev:i64,origin:i64,steps:Vec<i64>,runs:&[Vec<f64>]) -> Result<Self> {
        if steps_rev<=0 || origin<0 || steps.len()<2 || steps[0]!=0 || *steps.last().unwrap()!=steps_rev ||
            steps.windows(2).any(|w|w[1]<=w[0]) || runs.is_empty() ||
            runs.iter().any(|r|r.len()!=steps.len() || r.iter().any(|v|!v.is_finite())) {
            return Err("Meetcurve onvolledig of ongeldig".into());
        }
        let mut mean=vec![]; let mut sd=vec![];
        for i in 0..steps.len() {
            let values:Vec<_>=runs.iter().map(|r|r[i]-r[0]).collect();
            let (m,s)=stats(&values);mean.push(m);sd.push(s);
        }
        let mut blocks:Vec<(f64,usize)>=vec![];
        for value in &mean {
            blocks.push((value.max(0.),1));
            while blocks.len()>1 && blocks[blocks.len()-2].0>blocks[blocks.len()-1].0 {
                let b=blocks.pop().unwrap();let a=blocks.pop().unwrap();let n=a.1+b.1;
                blocks.push(((a.0*a.1 as f64+b.0*b.1 as f64)/n as f64,n));
            }
        }
        let masses:Vec<f64>=blocks.iter().flat_map(|(v,n)|std::iter::repeat_n(*v,*n)).collect();
        if *masses.last().unwrap()<=0. {return Err("Geen positieve uitstroom gemeten".into());}
        let correction_g=mean.iter().zip(&masses).map(|(a,b)|(a-b).abs()).fold(0.,f64::max);
        let interval_sd=(0..steps.len()).map(|i|if i==0{0.}else{stats(&runs.iter().map(|r|r[i]-r[i-1]).collect::<Vec<_>>()).1}).collect();
        Ok(Self{steps_rev,origin,steps,masses,raw_mean:mean,sd,interval_sd,runs:runs.to_vec(),correction_g})
    }
    pub fn yield_g(&self) -> f64 {*self.masses.last().unwrap()}
    pub fn cumulative(&self,position:i64) -> f64 {
        let cycles=position.div_euclid(self.steps_rev);let phase=position.rem_euclid(self.steps_rev);
        let i=self.steps.partition_point(|p|*p<=phase).saturating_sub(1).min(self.steps.len()-2);
        let f=(phase-self.steps[i]) as f64/(self.steps[i+1]-self.steps[i]) as f64;
        cycles as f64*self.yield_g()+self.masses[i]+f*(self.masses[i+1]-self.masses[i])
    }
    pub fn target(&self,start:i64,grams:f64) -> Result<i64> {
        if start<self.origin || start>=MAX_POSITION || !grams.is_finite() || grams<=0. {return Err("Ongeldig doel of startpunt".into());}
        let wanted=self.cumulative(start-self.origin)+grams;
        if !wanted.is_finite() || self.cumulative(MAX_POSITION-self.origin)<wanted {return Err("Positieteller vol; opnieuw refereren en meten".into());}
        // First integer position reaching mass; plateaus handled without inverse division.
        let mut lo=start+1;let mut hi=MAX_POSITION;
        while lo<hi {let mid=lo+(hi-lo)/2;if self.cumulative(mid-self.origin)>=wanted {hi=mid;} else {lo=mid+1;}}
        Ok(lo)
    }
}
pub fn stats(values:&[f64]) -> (f64,f64) {
    if values.is_empty() {return (0.,0.);}
    let mean=values.iter().sum::<f64>()/values.len() as f64;
    let sd=if values.len()>1 {(values.iter().map(|v|(v-mean).powi(2)).sum::<f64>()/(values.len()-1) as f64).sqrt()}else{0.};
    (mean,sd)
}

#[cfg(test)] mod tests {
    use super::*;
    #[test] fn correction_sign() {assert!((corrected_mass(5.,-1.).unwrap()-4.95).abs()<1e-10);assert_eq!(corrected_mass(5.,0.).unwrap(),5.);assert!(corrected_mass(5.,f64::NAN).is_err());assert!(corrected_mass(5.,21.).is_err());}
    #[test] fn absolute_rounding_no_drift() {let t=angle_targets(25600,10.).unwrap();assert_eq!(t.len(),36);assert_eq!(t[0].1,711);assert_eq!(t.last().unwrap().1,25600);assert!(angle_targets(10,1.).is_err());}
    #[test] fn step_grid_ends_on_full_revolution() {
        let t=step_targets(25600,1000).unwrap();assert_eq!(t.len(),26);assert_eq!(t[0].1,1000);assert_eq!(t[24].1,25000);assert_eq!(t.last().unwrap().1,25600);
        assert_eq!(Interval::Steps(25600).targets(25600).unwrap().len(),1);assert!(step_targets(25600,0).is_err());assert!(step_targets(25600,25601).is_err());
        assert_eq!(Interval::Degrees(10.).targets(25600).unwrap(),angle_targets(25600,10.).unwrap());
    }
    #[test] fn phase_aware_doses_cross_plateaus() {
        let c=Curve::fit(25600,51200,vec![0,8533,19200,21333,25600],&[vec![0.,0.,5.,6.,6.]]).unwrap();
        let start=51200+5*25600;let first=c.target(start,5.).unwrap();let second=c.target(first,5.).unwrap();
        assert_eq!(first-start,19200);assert!((second-first-23467).abs()<=1);
    }
    #[test] fn fit_keeps_raw_noise_and_spread() {
        let c=Curve::fit(100,0,vec![0,25,50,100],&[vec![0.,0.5,0.49,1.],vec![0.,0.52,0.5,1.1]]).unwrap();
        assert!(c.correction_g>0.);assert!(c.sd[3]>0.);assert!(c.masses.windows(2).all(|w|w[1]>=w[0]));assert_eq!(c.raw_mean[1],0.51);
    }
    #[test] fn overflow_and_bad_config_rejected() {
        let c=Curve::fit(25600,0,vec![0,25600],&[vec![0.,1.]]).unwrap();assert!(c.target(MAX_POSITION-1,5.).is_err());
        let mut cfg=Config::default();cfg.steps_rev=0;assert!(cfg.validate().is_err());
    }
}
