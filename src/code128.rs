//! Code 128 (own encoder, no dependency) for pot labels: code set C for an even number of digits (the
//! 10-digit pot codes), else code set B. Output as module widths, inline SVG, an HTML label sheet to print
//! from the browser, and ZPL for a network label printer (port 9100). The decoder is used by the tests to
//! prove the printed bars give back the code (bars → symbols → checksum → text).
use crate::model::Result;
use std::{io::Write,net::{TcpStream,ToSocketAddrs},path::{Path,PathBuf},time::Duration};

/// Bar/space widths (bar first) of the 107 symbols; 106 = stop (with its final 2-module bar).
const PATTERNS:[&str;107]=["212222","222122","222221","121223","121322","131222","122213","122312","132212","221213",
    "221312","231212","112232","122132","122231","113222","123122","123221","223211","221132","221231","213212","223112",
    "312131","311222","321122","321221","312212","322112","322211","212123","212321","232121","111323","131123","131321",
    "112313","132113","132311","211313","231113","231311","112133","112331","132131","113123","113321","133121","313121",
    "211331","231131","213113","213311","213131","311123","311321","331121","312113","312311","332111","314111","221411",
    "431111","111224","111422","121124","121421","141122","141221","112214","112412","122114","122411","142112","142211",
    "241211","221114","413111","241112","134111","111242","121142","121241","114212","124112","124211","411212","421112",
    "421211","212141","214121","412121","111143","111341","131141","114113","114311","411113","411311","113141","114131",
    "311141","411131","211412","211214","211232","2331112"];
const START_B:usize=104;const START_C:usize=105;const STOP:usize=106;

/// Symbol values including start and checksum, without stop.
pub fn symbols(text:&str)->Result<Vec<usize>>{
    if text.is_empty(){return Err("Lege code".into());}
    let digits=text.len()%2==0 && text.bytes().all(|b|b.is_ascii_digit());
    let mut v=if digits{
        let mut v=vec![START_C];for p in text.as_bytes().chunks(2){v.push(((p[0]-b'0')*10+(p[1]-b'0')) as usize);}v
    }else{
        let mut v=vec![START_B];for b in text.bytes(){if !(32..=126).contains(&b){return Err(format!("Teken niet in Code 128-B: {:?}",b as char));}v.push((b-32) as usize);}v
    };
    let sum=v.iter().enumerate().map(|(i,s)|if i==0{*s}else{s*i}).sum::<usize>()%103;
    v.push(sum);Ok(v)
}
/// Module widths of the whole symbol (bar, space, bar, …), stop included; no quiet zone.
pub fn widths(text:&str)->Result<Vec<u8>>{
    let mut v=symbols(text)?;v.push(STOP);
    Ok(v.iter().flat_map(|s|PATTERNS[*s].bytes().map(|b|b-b'0')).collect())
}
/// Inline SVG: `module` px per module, `height` px bars, quiet zone of 10 modules, the text under the bars.
pub fn svg(text:&str,module:f64,height:f64)->Result<String>{
    let w=widths(text)?;let total:u32=w.iter().map(|x|*x as u32).sum::<u32>()+20;
    let mut x=10u32;let mut bars=String::new();
    for (i,m) in w.iter().enumerate(){if i%2==0{bars+=&format!("<rect x=\"{}\" y=\"0\" width=\"{}\" height=\"{height}\"/>",x as f64*module,*m as f64*module);}x+=*m as u32;}
    let wpx=total as f64*module;
    Ok(format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{wpx}\" height=\"{}\" viewBox=\"0 0 {wpx} {}\" data-code=\"{}\"><rect width=\"100%\" height=\"100%\" fill=\"#fff\"/><g fill=\"#000\">{bars}</g><text x=\"{}\" y=\"{}\" font-family=\"monospace\" font-size=\"{}\" text-anchor=\"middle\">{}</text></svg>",
        height+16.,height+16.,esc(text),wpx/2.,height+13.,12,esc(text)))
}
fn esc(s:&str)->String{s.replace('&',"&amp;").replace('<',"&lt;").replace('>',"&gt;").replace('"',"&quot;")}

/// Decode module widths (bar first, stop included) back to text; checks start, checksum and stop.
pub fn decode_widths(w:&[u8])->Result<String>{
    if w.len()<6*3+7 || (w.len()-7)%6!=0{return Err("Ongeldige lengte".into());}
    let n=(w.len()-7)/6;
    let mut vals=vec![];
    for k in 0..n{let pat:String=w[k*6..k*6+6].iter().map(|d|(b'0'+d) as char).collect();vals.push(PATTERNS.iter().position(|p|*p==pat).ok_or(format!("Onbekend symbool {pat}"))?);}
    let stop:String=w[n*6..].iter().map(|d|(b'0'+d) as char).collect();if stop!=PATTERNS[STOP]{return Err("Geen stopteken".into());}
    let (body,check)=vals.split_at(vals.len()-1);
    let sum=body.iter().enumerate().map(|(i,s)|if i==0{*s}else{s*i}).sum::<usize>()%103;
    if sum!=check[0]{return Err(format!("Controlegetal klopt niet ({} ≠ {})",sum,check[0]));}
    match body[0]{
        START_C=>Ok(body[1..].iter().map(|v|format!("{v:02}")).collect()),
        START_B=>Ok(body[1..].iter().map(|v|((*v as u8)+32) as char).collect()),
        s=>Err(format!("Onverwacht startteken {s}")),
    }
}
/// Read the bars back from our SVG (rect x/width, module size `module`).
pub fn decode_svg(svg:&str,module:f64)->Result<String>{
    let mut bars=vec![];
    for part in svg.split("<rect ").skip(1){
        let attr=|k:&str|part.split(&format!("{k}=\"")).nth(1).and_then(|r|r.split('"').next()).and_then(|v|v.parse::<f64>().ok());
        if let (Some(x),Some(w))=(attr("x"),attr("width")){bars.push(((x/module).round() as i64,(w/module).round() as i64));}
    }
    if bars.is_empty(){return Err("Geen streepjes".into());}
    let mut w=vec![];
    for (i,(x,bw)) in bars.iter().enumerate(){
        if i>0{let (px,pw)=bars[i-1];w.push((x-px-pw) as u8);}
        w.push(*bw as u8);
    }
    decode_widths(&w)
}

/// One label: code plus up to three text lines.
#[derive(Clone,Debug)]
pub struct Label{pub code:String,pub lines:Vec<String>}
/// Printable A4 label sheet (3 columns), every code as inline SVG.
pub fn label_sheet(title:&str,labels:&[Label])->Result<String>{
    let mut cells=String::new();
    for l in labels{
        cells+=&format!("<div class=\"l\">{}<div class=\"t\">{}</div></div>",svg(&l.code,1.6,48.)?,l.lines.iter().map(|x|esc(x)).collect::<Vec<_>>().join("<br>"));
    }
    Ok(format!("<!doctype html><html lang=\"nl\"><head><meta charset=\"utf-8\"><title>{}</title><style>
@page{{size:A4;margin:10mm}}body{{font-family:Segoe UI,Arial,sans-serif;margin:0}}h1{{font-size:14px;margin:0 0 6px}}
.g{{display:grid;grid-template-columns:repeat(3,1fr);gap:4mm}}.l{{border:1px dashed #bbb;padding:3mm;text-align:center;break-inside:avoid}}
.t{{font-size:11px;margin-top:2px}}@media print{{h1,.p{{display:none}}.l{{border:none}}}}</style></head>
<body><h1>{}</h1><p class=\"p\"><button onclick=\"print()\">Afdrukken</button></p><div class=\"g\">{cells}</div></body></html>",esc(title),esc(title)))
}
/// Write the sheet to `dir` and return its path.
pub fn save_sheet(dir:&Path,name:&str,html:&str)->Result<PathBuf>{
    std::fs::create_dir_all(dir).map_err(|e|e.to_string())?;
    let p=dir.join(format!("{}.html",name.replace(['/','\\',':',' '],"_")));std::fs::write(&p,html).map_err(|e|e.to_string())?;Ok(p)
}
/// ZPL (Zebra) for labels: Code 128 with the code under it, then the text lines.
pub fn zpl(labels:&[Label])->String{
    labels.iter().map(|l|{
        let mut s=format!("^XA^CI28^FO40,30^BY2^BCN,90,Y,N,N^FD{}^FS",l.code);
        for (k,t) in l.lines.iter().take(3).enumerate(){s+=&format!("^FO40,{}^A0N,26,26^FD{}^FS",160+k*30,t.replace(['^','~'],""));}
        s+"^XZ\n"
    }).collect()
}
/// Send ZPL to a network printer (raw TCP, usually port 9100), with time-outs.
pub fn send_zpl(host:&str,port:u16,data:&str)->Result<()>{
    let addr=(host.trim(),port).to_socket_addrs().map_err(|e|format!("Printer {host}:{port}: {e}"))?.next().ok_or("Printer: geen adres")?;
    let mut s=TcpStream::connect_timeout(&addr,Duration::from_secs(3)).map_err(|e|format!("Printer {host}:{port}: {e}"))?;
    s.set_write_timeout(Some(Duration::from_secs(5))).ok();
    s.write_all(data.as_bytes()).and_then(|_|s.flush()).map_err(|e|format!("Printer: {e}"))
}

#[cfg(test)] mod tests{
    use super::*;
    #[test]fn table_is_valid(){
        for (i,p) in PATTERNS.iter().enumerate(){let s:u32=p.bytes().map(|b|(b-b'0') as u32).sum();assert_eq!(s,if i==STOP{13}else{11},"symbool {i}");}
        let mut u=PATTERNS.to_vec();u.sort();u.dedup();assert_eq!(u.len(),107,"alle patronen verschillend");
    }
    #[test]fn p7_codes_decode_from_our_svg(){
        for code in ["0000000001","0000012345","9999999999","AB-12 x","12345"]{
            let s=svg(code,1.6,48.).unwrap();assert_eq!(decode_svg(&s,1.6).unwrap(),code);
            assert_eq!(decode_widths(&widths(code).unwrap()).unwrap(),code);
        }
        // Pot code: start C, five digit pairs, checksum: 105 + 0·1 + 0·2 + 0·3 + 0·4 + 42·5 = 315 → 315 mod 103 = 6.
        assert_eq!(symbols("0000000042").unwrap(),vec![105,0,0,0,0,42,6]);
        // A wrong bar is caught by the checksum or the table.
        let mut w=widths("0000000042").unwrap();w[20]=if w[20]==1{2}else{1};w[21]=if w[21]==1{2}else{1};assert!(decode_widths(&w).is_err());
        assert!(symbols("").is_err() && symbols("é").is_err());
    }
    #[test]fn p7_label_sheet_and_zpl_to_fake_printer(){
        let labels=vec![Label{code:"0000000007".into(),lines:vec!["Order A".into(),"Recept v2 · pot 1/2".into()]},Label{code:"0000000008".into(),lines:vec!["Order A".into()]}];
        let html=label_sheet("Labels order A",&labels).unwrap();
        let codes:Vec<String>=html.split("<svg").skip(1).map(|s|decode_svg(&format!("<svg{s}"),1.6).unwrap()).collect();
        assert_eq!(codes,vec!["0000000007","0000000008"]);assert!(html.contains("Recept v2 · pot 1/2"));
        let dir=std::env::temp_dir().join(format!("pumpstudio-labels-{}",std::process::id()));let p=save_sheet(&dir,"labels order A",&html).unwrap();assert!(p.exists());
        let z=zpl(&labels);assert_eq!(z.matches("^XA").count(),2);assert!(z.contains("^BCN,90,Y,N,N^FD0000000007^FS"));
        let l=std::net::TcpListener::bind("127.0.0.1:0").unwrap();let port=l.local_addr().unwrap().port();
        let h=std::thread::spawn(move||{let (mut s,_)=l.accept().unwrap();let mut v=String::new();std::io::Read::read_to_string(&mut s,&mut v).unwrap();v});
        send_zpl("127.0.0.1",port,&z).unwrap();assert_eq!(h.join().unwrap(),z);
        let _=std::fs::remove_dir_all(dir);
    }
}
