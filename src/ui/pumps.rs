//! Pompen: all pumps of the active profile, filter, inline edit (kop, ingrediënt, actief; stored at
//! once) and manual control (VSD: dispense/move/home/zero/init/status; keramisch: forward only).
use super::*;
use pump_studio::{devices::{group,parse_decimal,parse_steps,steps_text,to_steps,DevId,Motion},machine::{check_steps,Head,PumpOp},workflow::{jog_target,Action,MAX_JOG_REVS}};

const ROW_H:f32=26.;
fn cell<R>(ui:&mut egui::Ui,w:f32,add:impl FnOnce(&mut egui::Ui)->R)->R{
    ui.allocate_ui_with_layout(egui::vec2(w,ROW_H),egui::Layout::left_to_right(egui::Align::Center),|ui|{ui.set_width(w);add(ui)}).inner
}
const COLS:[(&str,f32);8]=[("",22.),("Pomp",62.),("Kop",64.),("Node/motor",70.),("Ingrediënt",150.),("Actief",40.),("Positie",150.),("Kalibratie",80.)];

impl App{
    pub fn pumps_page(&mut self,ui:&mut egui::Ui){
        let vsd=self.profile()==ProfileKind::Vsd;
        page_title(ui,"Pompen",if vsd{"192 VSD-pompen: A/B op LH2 (borden 610–621), C/D op LH1 (borden 622–633), 8 motoren per bord. Kop, ingrediënt en actief direct aanpassen; wordt meteen opgeslagen."}
            else{"Keramische pompbank. Kop en ingrediënt direct aanpassen; CAN-adres en pomptype bij Instellingen."});
        let wide=ui.available_width()>=1040.;
        if wide{
            ui.horizontal_top(|ui|{
                let w=ui.available_width()-352.;
                ui.vertical(|ui|{ui.set_width(w);self.pump_table(ui);});
                ui.vertical(|ui|{ui.set_width(340.);if vsd{self.vsd_control(ui)}else{self.bench_control(ui)}});
            });
        }else{self.pump_table(ui);ui.add_space(8.);if vsd{self.vsd_control(ui)}else{self.bench_control(ui)}}
    }
    fn visible(&self)->Vec<usize>{
        let head=match self.fhead{1=>Some(Head::LH1),2=>Some(Head::LH2),_=>None};let active=match self.factive{1=>Some(true),2=>Some(false),_=>None};
        (0..self.pumps.len()).filter(|i|self.pumps[*i].matches(&self.filter,head,active)).collect()
    }
    fn pump_table(&mut self,ui:&mut egui::Ui){
        let vsd=self.profile()==ProfileKind::Vsd;
        card(ui,"",|ui|{
            ui.horizontal_wrapped(|ui|{
                ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("zoek pomp, ingrediënt of node").desired_width(190.));
                ui.label("Kop");for (k,l) in ["Alle","LH1","LH2"].iter().enumerate(){ui.selectable_value(&mut self.fhead,k,*l);}
                ui.separator();ui.label("Actief");for (k,l) in ["Alle","Aan","Uit"].iter().enumerate(){ui.selectable_value(&mut self.factive,k,*l);}
            });
            let visible=self.visible();
            ui.horizontal(|ui|{
                if vsd{
                    if ui.small_button("Zichtbare kiezen").clicked(){for i in &visible{self.sel.insert(self.pumps[*i].id);}}
                    if ui.small_button("Niets kiezen").clicked(){self.sel.clear();}
                }
                hint(ui,&format!("{} van {} pompen zichtbaar · {} gekozen · {} actief",visible.len(),self.pumps.len(),self.sel.len(),self.pumps.iter().filter(|p|p.enabled).count()));
            });
            ui.add_space(2.);
            egui::ScrollArea::horizontal().id_salt("pump-h").show(ui,|ui|{
                ui.horizontal(|ui|{for (t,w) in COLS{cell(ui,w,|ui|{ui.label(RichText::new(t).size(12.).strong().color(MUTED));});}});
                ui.separator();
                if visible.is_empty(){empty(ui,"Geen pompen bij dit filter.",80.);return;}
                let mut changed:Vec<PumpRow>=vec![];
                let height=(ui.available_height()-8.).clamp(260.,560.);
                egui::ScrollArea::vertical().id_salt("pump-rows").max_height(height).auto_shrink([false,true]).show_rows(ui,ROW_H+4.,visible.len(),|ui,range|{
                    for i in visible[range].iter().copied(){
                        let row=self.pumps[i].clone();let mut edit=row.clone();
                        ui.horizontal(|ui|{
                            cell(ui,COLS[0].1,|ui|{let mut s=self.sel.contains(&row.id);if ui.checkbox(&mut s,"").changed(){if s{self.sel.insert(row.id);}else{self.sel.remove(&row.id);}}});
                            cell(ui,COLS[1].1,|ui|{ui.label(RichText::new(&row.name).monospace().color(if row.enabled{INK}else{MUTED}));});
                            cell(ui,COLS[2].1,|ui|{
                                let heads:&[Option<Head>]=if vsd{&[Some(Head::LH1),Some(Head::LH2)]}else{&[None,Some(Head::LH1),Some(Head::LH2)]};
                                egui::ComboBox::from_id_salt(("head",row.id)).width(56.).selected_text(row.head.map(|h|h.name()).unwrap_or("—"))
                                    .show_ui(ui,|ui|{for h in heads{ui.selectable_value(&mut edit.head,*h,h.map(|h|h.name()).unwrap_or("—"));}});
                            });
                            cell(ui,COLS[3].1,|ui|{ui.label(RichText::new(format!("{} / {}",row.node,row.motor)).monospace());});
                            cell(ui,COLS[4].1,|ui|{
                                let buf=self.ing_buf.entry(row.id).or_insert_with(||row.ingredient.clone());
                                let r=ui.add(egui::TextEdit::singleline(buf).desired_width(92.).hint_text("—"));
                                // Commit on Enter or leaving the field; Esc cancels and restores the stored name.
                                let esc=ui.input(|i|i.key_pressed(egui::Key::Escape));
                                if let Some(v)=field_commit(r.lost_focus(),esc,buf,&row.ingredient){edit.ingredient=v;}
                                else if !r.has_focus() && *buf!=row.ingredient{*buf=row.ingredient.clone();}
                                // A list (popup), so Esc closes the list and is not taken as STOP.
                                egui::ComboBox::from_id_salt(("ing",row.id)).width(40.).selected_text("Kies").height(260.).show_ui(ui,|ui|{
                                    if ui.selectable_label(false,"(geen)").clicked(){edit.ingredient.clear();}
                                    for n in &self.ingredients{if ui.selectable_label(*n==row.ingredient,n).clicked(){edit.ingredient=n.clone();}}
                                }).response.on_hover_text("Kies een bestaand ingrediënt");
                            });
                            cell(ui,COLS[5].1,|ui|{ui.checkbox(&mut edit.enabled,"");});
                            let spr=if vsd{row.steps_rev.unwrap_or(self.dev.settings.machine.spr)}else{self.dev.settings.bench.steps_rev}.max(1);
                            cell(ui,COLS[6].1,|ui|{ui.label(RichText::new(match row.position{None=>"—".into(),Some(p)=>format!("{} st · {:.4} omw",group(p.round() as i64),p/spr as f64).replace('.',",")}).monospace());});
                            cell(ui,COLS[7].1,|ui|{
                                let session=!vsd && (self.dev.bench.curve.is_some() || self.dev.bench.calibration_g_rev.is_some());
                                // Date (dd-mm) of the active calibration; "(sim)" when only a simulation calibration exists.
                                let (t,c)=match self.cal_dates.get(&row.name){Some((ms,sim))=>{let d=applog::local_time(*ms);(format!("{}-{}{}",d.get(8..10).unwrap_or(""),d.get(5..7).unwrap_or(""),if *sim{" (sim)"}else{""}),if *sim{WARN}else{OK})},None if session=>("sessie".into(),WARN),None=>("nee".into(),MUTED)};
                                ui.label(RichText::new(t).color(c)).on_hover_text("Datum van de actieve kalibratie (database)");
                            });
                        });
                        if edit!=row{self.ing_buf.insert(row.id,edit.ingredient.clone());changed.push(edit);}
                    }
                });
                for r in changed{self.click(&format!("Pomp {} wijzigen",r.name));self.save_pump(r);}
            });
        });
    }
    fn selected_names(&self)->Vec<String>{self.pumps.iter().filter(|p|self.sel.contains(&p.id)).map(|p|p.name.clone()).collect()}
    fn vsd_control(&mut self,ui:&mut egui::Ui){
        let st=self.dev.status(DevId::Can);let ready=st.link==Link::On && !st.busy;
        let names=self.selected_names();
        let sel:Vec<PumpRow>=self.pumps.iter().filter(|p|self.sel.contains(&p.id)).cloned().collect();
        let spr_of=|p:&PumpRow|p.steps_rev.unwrap_or(self.dev.settings.machine.spr).max(1);
        let sprs:std::collections::BTreeSet<i64>=sel.iter().map(spr_of).collect();
        let spr=sprs.iter().next().copied().unwrap_or(self.dev.settings.machine.spr.max(1));
        card(ui,"Handbediening",|ui|{
            ui.horizontal(|ui|{dot(ui,link_color(st.link,st.simulation));ui.label(format!("CAN: {}",link_text(st.link,st.simulation)));if st.busy{ui.spinner();}});
            if names.is_empty(){empty(ui,"Kies één of meer pompen in de lijst.",56.);}
            else{
                let shown:Vec<&str>=names.iter().take(8).map(|s|s.as_str()).collect();
                ui.label(RichText::new(format!("{} gekozen: {}{}",names.len(),shown.join(", "),if names.len()>8{" …"}else{""})).strong());
                let off:Vec<&str>=sel.iter().filter(|p|!p.enabled).map(|p|p.name.as_str()).collect();
                if !off.is_empty(){ui.label(RichText::new(format!("Staat uit: {} (wordt geweigerd)",off.join(", "))).color(WARN));}
                for h in Head::ALL{
                    if !sel.iter().any(|p|p.head==Some(h)){continue;}
                    let s=self.dev.status(if h==Head::LH1{DevId::Lh1}else{DevId::Lh2});
                    hint(ui,&format!("Weegschaal {}: {}",h.name(),if s.link!=Link::On{"niet verbonden".into()}else{format!("{} netto{}",grams(s.weight.map(|w|w-s.tare)),if s.stable{" · stabiel"}else{" · beweegt"})}));
                }
            }
            section(ui,"DOSEREN / NAAR POSITIE");
            ui.horizontal(|ui|{
                for (k,l) in ["stappen","omwentelingen","graden"].iter().enumerate(){ui.selectable_value(&mut self.step_unit,k,*l);}
            });
            ui.horizontal(|ui|{
                let (buf,unit,tip)=match self.step_unit{0=>(&mut self.step_text,"st","Geheel getal; spatie of punt als duizendtal (25 600 / 25.600)"),1=>(&mut self.rev_text,"omw","Komma of punt als decimaal (0,5)"),_=>(&mut self.deg_text,"°","Komma of punt als decimaal (180 of 22,5)")};
                ui.add(egui::TextEdit::singleline(buf).desired_width(110.)).on_hover_text(tip);ui.label(unit);
                hint(ui,&format!("{} st/omw",group(spr)));});
            // Typed text -> exact steps; every refusal is shown and disables the buttons.
            let steps=if sprs.len()>1 && self.step_unit!=0{Err("Gekozen pompen hebben verschillende stappen/omw: kies eenheid stappen".to_string())}
                else{match self.step_unit{
                    0=>parse_steps(&self.step_text),
                    1=>parse_decimal(&self.rev_text).and_then(|r|to_steps(1,0,r,0.,spr)),
                    _=>parse_decimal(&self.deg_text).and_then(|d|to_steps(2,0,0.,d,spr)),
                }};
            hint(ui,if self.step_unit==0{"Stappen: geheel getal; punt of spatie is duizendtalscheiding (25.600 = 25 600 st)."}
                else{"Omwentelingen/graden: komma of punt is decimaal (1.000 = 1,0 — anders dan bij stappen)."});
            // A given confirmation of a move back is only valid for this unit, text and pump choice.
            let key=(self.step_unit,match self.step_unit{0=>self.step_text.clone(),1=>self.rev_text.clone(),_=>self.deg_text.clone()},{let mut v:Vec<i64>=self.sel.iter().copied().collect();v.sort();v});
            if key!=self.back_key{self.back_key=key;self.back_ok=false;}
            match &steps{Ok(v)=>{ui.label(RichText::new(format!("= {}",steps_text(*v,spr))).monospace());}Err(e)=>{ui.label(RichText::new(e).color(DANGER));}}
            let disp=steps.clone().and_then(|v|check_steps(PumpOp::Dispense,v).map(|_|v));
            let mv=steps.clone().and_then(|v|check_steps(PumpOp::MoveTo,v).map(|_|v));
            // Naar positie may go back (MOVETO); a move to a lower position needs an explicit confirmation.
            let back:Vec<String>=match &mv{Ok(t)=>sel.iter().filter(|p|p.position.is_some_and(|q|(q.round() as i64)>*t)).map(|p|p.name.clone()).collect(),Err(_)=>vec![]};
            if !back.is_empty(){ui.checkbox(&mut self.back_ok,RichText::new(format!("Terugbeweging bevestigen ({})",back.join(", "))).color(WARN));}
            ui.add_enabled_ui(ready && !names.is_empty(),|ui|{
                ui.horizontal_wrapped(|ui|{
                    if ui.add_enabled(disp.is_ok(),egui::Button::new(RichText::new("Doseren (+)").strong().color(Color32::WHITE)).fill(ACCENT))
                        .on_hover_text("Relatief vanaf de laatst bekende positie, alleen vooruit; eindigt met SETPOS naar de fase binnen de slag.").clicked(){if let Ok(v)=disp{self.vsd_cmd(names.clone(),PumpOp::Dispense,v);}}
                    if ui.add_enabled(mv.is_ok() && (back.is_empty() || self.back_ok),egui::Button::new("Naar positie"))
                        .on_hover_text("Absolute positie (MOVETO); mag terug na bevestiging.").clicked(){if let Ok(v)=mv{self.vsd_cmd(names.clone(),PumpOp::MoveTo,v);self.back_ok=false;}}
                });
                if let Err(e)=&disp{if steps.is_ok(){hint(ui,&format!("Doseren: {e}"));}}
            });
            section(ui,"SNELHEID");
            // Typed in steps like Camelot's log (s=51200 a=51200 d=51200); stored per profile, used from the next command.
            let m=&self.dev.settings.machine;let base=m.spr.max(1);
            for (k,v) in [m.speed_rps,m.accel_rps2,m.decel_rps2].into_iter().enumerate(){if self.motion_text[k].is_empty(){self.motion_text[k]=group((v*base as f64).round() as i64);}}
            let parsed:Vec<Result<i64,String>>=self.motion_text.iter().map(|t|parse_steps(t).and_then(|v|if v>0 && v<=20*base{Ok(v)}else{Err(format!("1 tot {} st/s",group(20*base)))})).collect();
            egui::Grid::new("vsd-motion").num_columns(3).spacing([10.,4.]).show(ui,|ui|{
                for (k,l) in ["Snelheid (st/s)","Acceleratie (st/s²)","Deceleratie (st/s²)"].iter().enumerate(){
                    ui.label(*l);ui.add(egui::TextEdit::singleline(&mut self.motion_text[k]).desired_width(90.));
                    match &parsed[k]{Ok(v)=>{ui.label(RichText::new(format!("{} omw/s",(*v as f64/base as f64)).replace('.',",")).color(MUTED));}Err(e)=>{ui.label(RichText::new(e).color(DANGER));}}
                    ui.end_row();
                }
            });
            let ok=parsed.iter().all(|p|p.is_ok());
            if ui.add_enabled(ok,egui::Button::new("Snelheid toepassen")).on_hover_text("Geldt vanaf de volgende pompopdracht, zonder opnieuw verbinden; wordt opgeslagen.").clicked(){
                let v:Vec<f64>=parsed.iter().map(|p|*p.as_ref().unwrap() as f64/base as f64).collect();
                self.click(&format!("Snelheid {} / {} / {} st/s",self.motion_text[0],self.motion_text[1],self.motion_text[2]));
                let r=self.dev.set_motion(Motion{speed_rps:v[0],accel_rps2:v[1],decel_rps2:v[2]});
                let r=r.and_then(|_|{let m=self.dev.settings.machine.clone();self.db.set_setting(ProfileKind::Vsd,"machine",&m)});
                match r{Ok(())=>self.message="Snelheid toegepast (vanaf volgende opdracht)".into(),Err(e)=>self.message=e}
            }
            section(ui,"HOME");
            ui.add_enabled_ui(ready && !names.is_empty(),|ui|{
                if ui.add(egui::Button::new(RichText::new(format!("Home gekozen pompen ({})",names.len())).strong().color(Color32::WHITE)).fill(ACCENT).min_size(egui::vec2(220.,30.)))
                    .on_hover_text("HOME (22) naar de homesensor, met de snelheid hierboven; daarna is de positie 0.").clicked(){self.vsd_cmd(names.clone(),PumpOp::Home,0);}
                ui.horizontal_wrapped(|ui|{for (l,op) in [("Zero",PumpOp::Zero),("Init",PumpOp::Init),("Status",PumpOp::Status)]{if ui.button(l).clicked(){self.vsd_cmd(names.clone(),op,0);}}});
            });
            if st.link!=Link::On{hint(ui,"Eerst CAN verbinden (Instellingen > Verbindingen).");}
            else{ui.add_space(4.);result_line(ui,&st.text,st.ok);}
            // What the last move did, per pump (exact steps as sent).
            for p in sel.iter().take(4){
                if let Some(m)=st.moves.get(&p.name){
                    let sp=m.spr.max(1);
                    let mut t=format!("{} {}: voor {} · doel (p1) {} st · bereikt {} st",p.name,m.op,m.before.map(|b|format!("{} st",group(b))).unwrap_or("onbekend".into()),group(m.target),group(m.reached));
                    if let Some(ph)=m.phase{t+=&format!(" · na SETPOS {} st (fase binnen de slag, {:.1}°)",group(ph),(ph as f64/sp as f64*360.)).replace('.',",");}
                    hint(ui,&t);
                }
            }
            hint(ui,"Doseren telt op bij de laatst bekende positie en zet de teller daarna terug naar de fase binnen de slag (SETPOS, zoals Camelot); de absolute positie springt dus terug. Gekozen pompen draaien tegelijk. STOP / Esc onderbreekt binnen 1 s.");
        });
    }
    fn vsd_cmd(&mut self,names:Vec<String>,op:PumpOp,steps:i64){
        self.click(&format!("Pomp {op:?} {} st: {}",group(steps),names.join(", ")));
        let r=self.dev.pumps(names,op,steps);self.result(r);
    }
    fn bench_action(&mut self,what:&str,a:Action){self.click(what);let r=self.dev.bench_action(a);self.result(r);}
    fn bench_control(&mut self,ui:&mut egui::Ui){
        let b=self.dev.bench.clone();let st=self.dev.status(DevId::Bench);let ready=b.connected && !st.busy;
        card(ui,"Handbediening keramische pomp",|ui|{
            ui.horizontal(|ui|{dot(ui,link_color(st.link,st.simulation));ui.label(format!("Pompbank: {}",link_text(st.link,st.simulation)));if st.busy{ui.spinner();}});
            ui.horizontal(|ui|{
                ui.label(RichText::new(format!("{} st",b.position)).size(20.).strong().monospace());
                ui.label(RichText::new(if b.reference_valid{"referentie actief"}else{"referentie ongeldig"}).color(if b.reference_valid{OK}else{DANGER}));
            });
            if b.rebases>0{hint(ui,&format!("controller {} st · {} tellerreset(s)",b.position-b.controller_offset,b.rebases));}
            ui.add_enabled_ui(ready,|ui|{ui.horizontal_wrapped(|ui|{
                if ui.button("Huidige stand als startpositie 0").clicked(){self.bench_action("Startpositie 0",Action::Zero);}
                if ui.button("Home via sensor").clicked(){self.bench_action("Home via sensor",Action::Home);}
            });});
            section(ui,"VOORUIT");
            let n=if b.connected{b.config.steps_rev}else{self.dev.settings.bench.steps_rev};
            let origin=b.curve.as_ref().map(|c|c.origin).unwrap_or(b.reference_origin);
            ui.horizontal(|ui|{for (k,l) in ["Relatief","Naar teller","Naar fase"].iter().enumerate(){ui.selectable_value(&mut self.jog_mode,k,*l);}});
            ui.horizontal(|ui|match self.jog_mode{
                0=>{ui.add(egui::DragValue::new(&mut self.jog_amount).speed(0.1).max_decimals(3));for (k,l) in ["stappen","omw","graden"].iter().enumerate(){ui.selectable_value(&mut self.jog_unit,k,*l);}}
                1=>{ui.label("Teller");ui.add(egui::DragValue::new(&mut self.jog_target).speed(100.));}
                _=>{ui.label("Fase");ui.add(egui::DragValue::new(&mut self.jog_phase).range(0. ..=359.9).suffix(" °"));}
            });
            let value=match self.jog_mode{0=>self.jog_amount,1=>self.jog_target as f64,_=>self.jog_phase};
            let target=jog_target(b.position,n,origin,self.jog_mode,self.jog_unit,value);
            match &target{Ok(t)=>hint(ui,&format!("{} -> {t} · +{} st = {:.3} omw",b.position,t-b.position,(t-b.position) as f64/n as f64)),Err(e)=>{ui.label(RichText::new(e).color(DANGER));}}
            ui.checkbox(&mut self.jog_weigh,"Wegen voor en na");
            ui.add_enabled_ui(ready && b.reference_valid,|ui|{
                ui.horizontal_wrapped(|ui|{
                    if primary(ui,target.is_ok(),"Vooruit"){if let Ok(t)=target{self.bench_action(&format!("Vooruit naar {t}"),Action::Jog{target:t,weigh:self.jog_weigh});}}
                    for (l,steps) in [("+100 st",100.),("+1000 st",1000.),("+¼",n as f64/4.),("+½",n as f64/2.),("+1 omw",n as f64)]{
                        if ui.button(l).clicked(){if let Ok(t)=jog_target(b.position,n,origin,0,0,steps){self.bench_action(&format!("Vooruit {l}"),Action::Jog{target:t,weigh:self.jog_weigh});}}
                    }
                });
            });
            ui.add_enabled_ui(ready && b.reference_valid && b.scale_ok,|ui|{if ui.button("Weegschaal lezen").clicked(){self.bench_action("Weegschaal lezen",Action::Weigh);}});
            if !b.connected{hint(ui,"Eerst pompbank verbinden (Instellingen > Verbindingen).");}
            else if !b.reference_valid{hint(ui,"Eerst startpositie 0 of home kiezen.");}
            else if !b.scale_ok{hint(ui,"Geen weegschaal verbonden: wegen staat uit.");}
            ui.add_space(4.);result_line(ui,&b.status,!b.status.starts_with("Afgebroken"));
            if let Some(m)=b.manual.last(){
                hint(ui,&format!("Laatste: {} · {} -> {} st · {}{}",m.kind,m.start_steps,m.end_steps,m.status,m.delta_g.map(|d|format!(" · gewogen {d:.5} g")).unwrap_or_default()));
            }
            hint(ui,&format!("Alleen vooruit, maximaal {MAX_JOG_REVS} omwentelingen per opdracht; tellerbeveiliging actief. Fout of STOP maakt de referentie ongeldig."));
        });
    }
}
