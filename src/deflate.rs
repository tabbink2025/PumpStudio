//! Small DEFLATE compressor (RFC 1951, fixed Huffman codes, LZ77 with a hash chain) for the .xlsx export, so a
//! large workbook stays small without an extra dependency. `inflate` (fixed-Huffman blocks only) checks it in tests.

struct Bits{out:Vec<u8>,acc:u64,n:u32}
impl Bits{
    fn put(&mut self,v:u32,len:u32){self.acc|=(v as u64)<<self.n;self.n+=len;while self.n>=8{self.out.push(self.acc as u8);self.acc>>=8;self.n-=8;}}
    /// Huffman codes go most significant bit first.
    fn code(&mut self,code:u32,len:u32){let mut r=0;for i in 0..len{r|=((code>>i)&1)<<(len-1-i);}self.put(r,len);}
    fn finish(mut self)->Vec<u8>{if self.n>0{self.out.push(self.acc as u8);}self.out}
}
fn lit(b:&mut Bits,v:u32){
    match v{0..=143=>b.code(0x30+v,8),144..=255=>b.code(0x190+v-144,9),256..=279=>b.code(v-256,7),_=>b.code(0xC0+v-280,8)}
}
const LBASE:[u32;29]=[3,4,5,6,7,8,9,10,11,13,15,17,19,23,27,31,35,43,51,59,67,83,99,115,131,163,195,227,258];
const LEXT:[u32;29]=[0,0,0,0,0,0,0,0,1,1,1,1,2,2,2,2,3,3,3,3,4,4,4,4,5,5,5,5,0];
const DBASE:[u32;30]=[1,2,3,4,5,7,9,13,17,25,33,49,65,97,129,193,257,385,513,769,1025,1537,2049,3073,4097,6145,8193,12289,16385,24577];
const DEXT:[u32;30]=[0,0,0,0,1,1,2,2,3,3,4,4,5,5,6,6,7,7,8,8,9,9,10,10,11,11,12,12,13,13];

pub fn deflate(data:&[u8])->Vec<u8>{
    let mut b=Bits{out:Vec::with_capacity(data.len()/4+16),acc:0,n:0};
    b.put(1,1);b.put(1,2); // final block, fixed Huffman
    const W:usize=32768;const HB:usize=15;
    let mut head=vec![usize::MAX;1<<HB];let mut prev=vec![usize::MAX;W];
    let hash=|i:usize|((data[i] as usize)<<10^(data[i+1] as usize)<<5^data[i+2] as usize)&((1<<HB)-1);
    let mut i=0;
    while i<data.len(){
        let (mut best,mut dist)=(0usize,0usize);
        if i+3<=data.len(){
            let h=hash(i);let mut c=head[h];let mut chain=32;
            while c!=usize::MAX && i-c<=W && chain>0{
                let max=(data.len()-i).min(258);let mut l=0;while l<max && data[c+l]==data[i+l]{l+=1;}
                if l>best{best=l;dist=i-c;if l==max{break;}}
                c=prev[c%W];chain-=1;
            }
            prev[i%W]=head[h];head[h]=i;
        }
        if best>=3{
            let k=LBASE.iter().rposition(|x|*x as usize<=best).unwrap();lit(&mut b,257+k as u32);b.put((best-LBASE[k] as usize) as u32,LEXT[k]);
            let d=DBASE.iter().rposition(|x|*x as usize<=dist).unwrap();b.code(d as u32,5);b.put((dist-DBASE[d] as usize) as u32,DEXT[d]);
            // Insert the skipped positions into the hash chain.
            for j in i+1..(i+best).min(data.len().saturating_sub(2)){let h=hash(j);prev[j%W]=head[h];head[h]=j;}
            i+=best;
        }else{lit(&mut b,data[i] as u32);i+=1;}
    }
    lit(&mut b,256);b.finish()
}

/// Decoder for fixed-Huffman (and stored) blocks, enough to read back `deflate` output in tests.
pub fn inflate(src:&[u8])->Result<Vec<u8>,String>{
    let mut pos=0usize;let mut bit=0u32;let mut out:Vec<u8>=vec![];
    let get=|n:u32,pos:&mut usize,bit:&mut u32|->Result<u32,String>{let mut v=0;for i in 0..n{let byte=*src.get(*pos).ok_or("einde data")?;v|=(((byte>>*bit)&1) as u32)<<i;*bit+=1;if *bit==8{*bit=0;*pos+=1;}}Ok(v)};
    loop{
        let last=get(1,&mut pos,&mut bit)?;let ty=get(2,&mut pos,&mut bit)?;
        if ty!=1{return Err(format!("bloktype {ty} niet ondersteund"));}
        loop{
            // Read a fixed-Huffman symbol MSB first.
            let mut code=0u32;let mut len=0;let sym;
            loop{code=(code<<1)|get(1,&mut pos,&mut bit)?;len+=1;
                match len{7 if code<=0x17=>{sym=code+256;break}8 if (0x30..=0xBF).contains(&code)=>{sym=code-0x30;break}8 if (0xC0..=0xC7).contains(&code)=>{sym=code-0xC0+280;break}
                    9 if code>=0x190=>{sym=code-0x190+144;break}9=>return Err("ongeldige code".into()),_=>{}}}
            if sym<256{out.push(sym as u8);continue;}
            if sym==256{break;}
            let k=(sym-257) as usize;let l=LBASE[k] as usize+get(LEXT[k],&mut pos,&mut bit)? as usize;
            let mut d=0u32;for _ in 0..5{d=(d<<1)|get(1,&mut pos,&mut bit)?;}
            let dist=DBASE[d as usize] as usize+get(DEXT[d as usize],&mut pos,&mut bit)? as usize;
            if dist>out.len(){return Err("afstand te groot".into());}
            for _ in 0..l{out.push(out[out.len()-dist]);}
        }
        if last==1{return Ok(out);}
    }
}

#[cfg(test)] mod tests{
    use super::*;
    #[test]fn round_trip(){
        for d in [b"".to_vec(),b"a".to_vec(),b"abcabcabcabcabcabc hello hello hello".to_vec(),(0..70000u32).map(|i|(i*7%251) as u8).collect(),
            "<c r=\"A1\" t=\"s\"><v>1</v></c>".repeat(5000).into_bytes()]{
            let z=deflate(&d);assert_eq!(inflate(&z).unwrap(),d);
        }
        let x="<row><c><v>1.005</v></c></row>".repeat(10000);let z=deflate(x.as_bytes());assert!(z.len()*20<x.len(),"{} → {}",x.len(),z.len());
    }
}
