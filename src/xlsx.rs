//! Minimal .xlsx (OOXML SpreadsheetML) writer: several sheets, inline strings, numbers as numbers, dates as Excel
//! date serials with a date format. Packed with the stored zip of `report.rs` (no extra dependency).
use crate::model::Result;

#[derive(Clone,Debug,PartialEq)]
pub enum Cell{Text(String),Num(f64),Int(i64),
    /// Milliseconds since the epoch, written as a local date/time (Excel serial + format dd-mm-yyyy hh:mm).
    Date(i64),Empty}
pub struct Sheet{pub name:String,pub header:Vec<String>,pub rows:Vec<Vec<Cell>>}

fn esc(s:&str)->String{
    s.chars().filter(|c|!c.is_control() || *c=='\t' || *c=='\n').collect::<String>().replace('&',"&amp;").replace('<',"&lt;").replace('>',"&gt;").replace('"',"&quot;")
}
/// Column letters: 0 → A, 25 → Z, 26 → AA.
pub fn col(mut i:usize)->String{let mut s=String::new();loop{s.insert(0,(b'A'+(i%26) as u8) as char);if i<26{break;}i=i/26-1;}s}
/// Excel serial day number (1900 system) of a local time.
pub fn excel_serial(ms:i64)->f64{
    // Local wall clock from the app's own CET/CEST conversion, then days since 1899-12-30.
    let t=crate::applog::local_stamp(ms);
    let p=|a:usize,b:usize|t.get(a..b).and_then(|x|x.parse::<i64>().ok()).unwrap_or(0);
    let (y,m,d,h,mi,s,mil)=(p(0,4),p(5,7),p(8,10),p(11,13),p(14,16),p(17,19),p(20,23));
    let yy=if m<=2{y-1}else{y};let era=yy.div_euclid(400);let yoe=yy-era*400;let doy=(153*(m+if m>2{-3}else{9})+2)/5+d-1;
    let days=era*146097+yoe*365+yoe/4-yoe/100+doy-719468;
    (days+25569) as f64+(h*3600+mi*60+s) as f64/86400.+mil as f64/86_400_000.
}

pub fn workbook(sheets:&[Sheet])->Result<Vec<u8>>{
    if sheets.is_empty(){return Err("Geen tabbladen".into());}
    let mut files:Vec<(String,Vec<u8>)>=vec![];
    let mut sst=Sst::default();
    let mut ct=String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.styles+xml"/>"#);
    let mut wb=String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets>"#);
    let mut rels=String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#);
    for (k,sh) in sheets.iter().enumerate(){
        let n=k+1;
        ct+=&format!(r#"<Override PartName="/xl/worksheets/sheet{n}.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>"#);
        // Sheet names: max 31 characters, no []:*?/\
        let name:String=sh.name.chars().filter(|c|!"[]:*?/\\".contains(*c)).take(31).collect();
        wb+=&format!(r#"<sheet name="{}" sheetId="{n}" r:id="rId{n}"/>"#,esc(&name));
        rels+=&format!(r#"<Relationship Id="rId{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet{n}.xml"/>"#);
        files.push((format!("xl/worksheets/sheet{n}.xml"),sheet_xml(sh,&mut sst).into_bytes()));
    }
    let n=sheets.len()+1;
    rels+=&format!(r#"<Relationship Id="rId{n}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>"#);
    rels+=&format!(r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/sharedStrings" Target="sharedStrings.xml"/></Relationships>"#,n+1);
    ct+=r#"<Override PartName="/xl/sharedStrings.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sharedStrings+xml"/>"#;
    let mut ss=format!(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><sst xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" count="{}" uniqueCount="{}">"#,sst.count,sst.list.len());
    for t in &sst.list{ss+=&format!(r#"<si><t xml:space="preserve">{}</t></si>"#,esc(t));}
    ss+="</sst>";files.push(("xl/sharedStrings.xml".into(),ss.into_bytes()));
    wb+="</sheets></workbook>";ct+="</Types>";
    let styles=r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><styleSheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><numFmts count="1"><numFmt numFmtId="164" formatCode="dd-mm-yyyy hh:mm"/></numFmts><fonts count="2"><font><sz val="11"/><name val="Calibri"/></font><font><b/><sz val="11"/><name val="Calibri"/></font></fonts><fills count="2"><fill><patternFill patternType="none"/></fill><fill><patternFill patternType="gray125"/></fill></fills><borders count="1"><border><left/><right/><top/><bottom/><diagonal/></border></borders><cellStyleXfs count="1"><xf numFmtId="0" fontId="0" fillId="0" borderId="0"/></cellStyleXfs><cellXfs count="3"><xf numFmtId="0" fontId="0" fillId="0" borderId="0" xfId="0"/><xf numFmtId="164" fontId="0" fillId="0" borderId="0" xfId="0" applyNumberFormat="1"/><xf numFmtId="0" fontId="1" fillId="0" borderId="0" xfId="0" applyFont="1"/></cellXfs><cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles><dxfs count="0"/><tableStyles count="0" defaultTableStyle="TableStyleMedium2" defaultPivotStyle="PivotStyleLight16"/></styleSheet>"#;
    let root=r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#;
    let mut all=vec![("[Content_Types].xml".to_string(),ct.into_bytes()),("_rels/.rels".to_string(),root.as_bytes().to_vec()),
        ("xl/workbook.xml".to_string(),wb.into_bytes()),("xl/_rels/workbook.xml.rels".to_string(),rels.into_bytes()),("xl/styles.xml".to_string(),styles.as_bytes().to_vec())];
    all.extend(files);
    Ok(crate::report::zip_deflate(&all))
}
/// Shared strings: every distinct text once (ingredient, pump, status … repeat a lot in big exports).
#[derive(Default)]
struct Sst{list:Vec<String>,idx:std::collections::HashMap<String,usize>,count:usize}
/// Excel's limit for the text of one cell.
pub const MAX_CELL_CHARS:usize=32_767;
impl Sst{fn id(&mut self,t:&str)->usize{
    let t:&str=match t.char_indices().nth(MAX_CELL_CHARS){Some((i,_))=>&t[..i],None=>t};
    self.count+=1;if let Some(i)=self.idx.get(t){return *i;}let i=self.list.len();self.list.push(t.to_string());self.idx.insert(t.to_string(),i);i}}
fn sheet_xml(sh:&Sheet,sst:&mut Sst)->String{
    let mut x=String::from(r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetViews><sheetView workbookViewId="0"><pane ySplit="1" topLeftCell="A2" activePane="bottomLeft" state="frozen"/></sheetView></sheetViews><sheetData>"#);
    x+=r#"<row r="1">"#;
    for (c,h) in sh.header.iter().enumerate(){x+=&format!(r#"<c r="{}1" t="s" s="2"><v>{}</v></c>"#,col(c),sst.id(h));}
    x+="</row>";
    for (r,row) in sh.rows.iter().enumerate(){
        let rn=r+2;x+=&format!(r#"<row r="{rn}">"#);
        for (c,v) in row.iter().enumerate(){
            let at=format!("{}{rn}",col(c));
            match v{
                Cell::Text(t)=>x+=&format!(r#"<c r="{at}" t="s"><v>{}</v></c>"#,sst.id(t)),
                Cell::Num(f) if f.is_finite()=>x+=&format!(r#"<c r="{at}"><v>{f}</v></c>"#),
                Cell::Int(i)=>x+=&format!(r#"<c r="{at}"><v>{i}</v></c>"#),
                Cell::Date(ms)=>x+=&format!(r#"<c r="{at}" s="1"><v>{}</v></c>"#,excel_serial(*ms)),
                _=>{}
            }
        }
        x+="</row>";
    }
    x+="</sheetData></worksheet>";x
}

#[cfg(test)] pub(crate) mod tests{
    use super::*;
    use crate::report::tests::{unzip,well_formed};
    /// Shared strings of a workbook (index → text).
    pub fn strings(sst:&str)->Vec<String>{
        sst.split("<si>").skip(1).map(|p|{let t=&p[p.find('>').unwrap()+1..];t[..t.find("</t>").unwrap()].replace("&amp;","&").replace("&lt;","<").replace("&gt;",">").replace("&quot;","\"")}).collect()
    }
    /// Read a sheet back: (cell, value) with shared strings resolved.
    pub fn cells(xml:&str,sst:&[String])->Vec<(String,String)>{
        let mut out=vec![];
        for part in xml.split("<c r=\"").skip(1){
            let r=part.split('"').next().unwrap().to_string();
            let v=part.split("<v>").nth(1).unwrap();let v=v[..v.find("</v>").unwrap()].to_string();
            let head=&part[..part.find('>').unwrap()];
            out.push((r,if head.contains("t=\"s\""){sst[v.parse::<usize>().unwrap()].clone()}else{v}));
        }
        out
    }
    #[test]fn n2_workbook_structure_and_values(){
        assert_eq!((col(0).as_str(),col(25).as_str(),col(26).as_str(),col(27).as_str()),("A","Z","AA","AB"));
        // 2026-11-03 12:00 CET = 1793790000000 ms → serial 46329,5.
        assert!((excel_serial(1_793_703_600_000)-46329.5).abs()<1e-9,"{}",excel_serial(1_793_703_600_000));
        let b=workbook(&[Sheet{name:"Potten".into(),header:vec!["Code".into(),"g".into(),"Datum".into()],rows:vec![vec![Cell::Text("0000000001 <&>".into()),Cell::Num(1.25),Cell::Date(1_793_703_600_000)]]},
            Sheet{name:"Ingrediënten".into(),header:vec!["n".into()],rows:vec![vec![Cell::Int(42)]]}]).unwrap();
        let files=unzip(&b);let names:Vec<&str>=files.iter().map(|f|f.0.as_str()).collect();
        for n in ["[Content_Types].xml","_rels/.rels","xl/workbook.xml","xl/_rels/workbook.xml.rels","xl/styles.xml","xl/sharedStrings.xml","xl/worksheets/sheet1.xml","xl/worksheets/sheet2.xml"]{assert!(names.contains(&n),"{n}");}
        for (n,d) in &files{well_formed(std::str::from_utf8(d).unwrap()).unwrap_or_else(|e|panic!("{n}: {e}"));}
        let wb=String::from_utf8(files[2].1.clone()).unwrap();assert!(wb.contains(r#"name="Potten""#) && wb.contains(r#"name="Ingrediënten""#));
        let s1=String::from_utf8(files.iter().find(|f|f.0=="xl/worksheets/sheet1.xml").unwrap().1.clone()).unwrap();
        let sst=strings(std::str::from_utf8(&files.iter().find(|f|f.0=="xl/sharedStrings.xml").unwrap().1).unwrap());
        assert_eq!(cells(&s1,&sst),vec![("A1".into(),"Code".into()),("B1".into(),"g".into()),("C1".into(),"Datum".into()),("A2".into(),"0000000001 <&>".into()),("B2".into(),"1.25".into()),("C2".into(),"46329.5".into())]);
        assert!(s1.contains(r#"<c r="C2" s="1">"#),"datum met datumopmaak");
        let st=String::from_utf8(files.iter().find(|f|f.0=="xl/styles.xml").unwrap().1.clone()).unwrap();
        let (a,b,c)=(st.find("</cellXfs>").unwrap(),st.find(r#"<cellStyles count="1"><cellStyle name="Normal" xfId="0" builtinId="0"/></cellStyles>"#).expect("cellStyles"),st.find("<dxfs").unwrap());assert!(a<b && b<c,"volgorde styles.xml");
        // Text longer than Excel's cell limit is cut to 32 767 characters (é = 2 bytes: counted in characters).
        let long="é".repeat(40_000);let b=workbook(&[Sheet{name:"T".into(),header:vec!["x".into()],rows:vec![vec![Cell::Text(long)]]}]).unwrap();
        let f=unzip(&b);let ss=strings(std::str::from_utf8(&f.iter().find(|x|x.0=="xl/sharedStrings.xml").unwrap().1).unwrap());
        assert_eq!(ss[1].chars().count(),MAX_CELL_CHARS);
    }
}
