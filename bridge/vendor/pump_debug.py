"""Minimal pump workflow: calibration, characterization, phase-aware dosing.

Original CAN encoder, connection and scale methods live in pump_debug_legacy.
"""
import json
import csv
import io
import threading
from datetime import datetime
from pathlib import Path
import tkinter as tk
from tkinter import ttk,messagebox,filedialog

from pump_debug_legacy import PumpDebugApp as LegacyApp,COMMANDS
from ceramic_pump import Measurements,MeasurementPolicy,PumpConfig,integer,atomic_text
from pump_counter import CommandCounterAdapter,position_binding
from pump_results import normalize,records_from_payload
import pump_debug_characterization as characterization_ui

# Existing characterization panel creates this adapter for its normal workflow.
characterization_ui.CharacterizationAdapter=CommandCounterAdapter


class PumpDebugApp(LegacyApp):
    def _build_ui(self):
        self.counter_valid=False
        self.counter_bus=None
        self.counter_context=None
        self.wait_done_var=tk.BooleanVar(value=False)
        super()._build_ui()
        self.root.title('Pump Debug — kalibratie, curve en doseren')
        self.root.geometry('1220x960')
        self.motor.set('2')
        self.fo_init.set(False)
        self.fo_prime_n.set('0')
        tests=self.gravimetric_tests
        tests.fields['steps_rev']=self.fo_steps_rev
        tests.fields['fluid']=self.fo_fluid
        tests.config=self.workflow_config
        panel=tests.characterization
        notebook=tests.page.master
        calibration_page=None
        for page in list(notebook.tabs()):
            title=notebook.tab(page,'text')
            if title=='Forward-only':
                calibration_page=self.root.nametowidget(page)
                notebook.tab(page,text='1. Forward-only kalibratie')
            elif title=='360° Characterization':
                notebook.tab(page,text='2. 360° meetcurve')
            else:notebook.forget(page)
        # Keep original calibration fields/results, hide learning and trim paths.
        for child in calibration_page.winfo_children():
            if isinstance(child,ttk.LabelFrame):
                title=child.cget('text')
                if title.startswith(('2.','3.')) or title=='Forward-only doseerpomp':child.pack_forget()
                if title=='1. Kalibratie':
                    for frame in child.winfo_children():
                        for widget in frame.winfo_children():
                            if isinstance(widget,ttk.Checkbutton):widget.pack_forget()
                            if isinstance(widget,ttk.Button) and widget.cget('text')=='Profiel laden':widget.pack_forget()
        self.fo_cal_lbl.configure(text='Kalibratie meet g/omwenteling bij ingestelde mechanische slag.')
        self.fo_status_lbl.configure(text='Kies startpositie. Kalibreer, meet curve, doseer vanuit teller.')
        self.fo_primed_lbl.pack_forget()
        self.fo_status_lbl.master.master.pack_forget()
        dose_page=ttk.Frame(notebook,padding=8)
        notebook.add(dose_page,text='3. Doseren vanuit curve')
        # Hide original nested dose box; create same bound controls on third page.
        char_page=notebook.nametowidget(notebook.tabs()[1])
        for child in char_page.winfo_children():
            if isinstance(child,ttk.LabelFrame):child.grid_remove()
            if isinstance(child,ttk.Combobox) and str(child.cget('textvariable'))==str(panel.reference_choice):
                child.grid_remove()
        panel.reference_choice.set('Relatieve startpositie (zonder homesensor)')
        ttk.Label(dose_page,text='Gewenste massa (g)').grid(row=0,column=0,sticky='w')
        ttk.Entry(dose_page,textvariable=panel.curve_target,width=14).grid(row=0,column=1)
        ttk.Label(dose_page,text='Herhalingen').grid(row=1,column=0,sticky='w')
        ttk.Entry(dose_page,textvariable=panel.curve_repeats,width=14).grid(row=1,column=1)
        self.correction_enabled=tk.BooleanVar(value=False)
        self.correction_percent=tk.StringVar(value='0')
        correction=ttk.LabelFrame(dose_page,text='Handmatige correctie — minder (-) / meer (+)',padding=4)
        correction.grid(row=2,column=0,columnspan=4,sticky='ew',pady=6)
        ttk.Checkbutton(correction,text='Correctie gebruiken',variable=self.correction_enabled).pack(side='left',padx=4)
        ttk.Entry(correction,textvariable=self.correction_percent,width=8).pack(side='left')
        ttk.Label(correction,text='%').pack(side='left',padx=3)
        for label,delta in (('-1%',-1),('-0,5%',-.5),('+0,5%',.5),('+1%',1)):
            ttk.Button(correction,text=label,command=lambda d=delta:self.adjust_correction(d)).pack(side='left',padx=3)
        ttk.Button(correction,text='Reset 0%',command=self.reset_correction).pack(side='left',padx=3)
        ttk.Button(dose_page,text='Bereken verplaatsing',command=self.preview_dose).grid(row=3,column=0,pady=8)
        ttk.Button(dose_page,text='Doseren',command=panel.start_curve_dose).grid(row=3,column=1,pady=8)
        ttk.Button(dose_page,text='STOP',command=panel.stop).grid(row=3,column=2,padx=8)
        self.dose_preview=tk.StringVar(value='Curve nodig; elke volgende dosis rekent door vanaf huidige teller.')
        ttk.Label(dose_page,textvariable=self.dose_preview,wraplength=1100).grid(row=4,column=0,columnspan=4,sticky='w')
        ttk.Label(dose_page,textvariable=panel.curve_status,wraplength=1100).grid(row=5,column=0,columnspan=4,sticky='w',pady=8)
        ttk.Label(dose_page,text='Geen automatische prime, trim, HOME of ZERO tussen doses. Schaal controleert resultaat.').grid(row=6,column=0,columnspan=4,sticky='w')
        self.dose_last_result=tk.StringVar(value='Nog geen doseermeting.')
        ttk.Label(dose_page,textvariable=self.dose_last_result,wraplength=1100,font=('Segoe UI',12)).grid(row=7,column=0,columnspan=4,sticky='w',pady=12)
        setup=ttk.LabelFrame(self.root,text='Pomp en gedeelde positieteller',padding=6)
        setup.pack(fill='x',padx=8,pady=4,before=notebook)
        for col,(label,var,width) in enumerate((('Node',self.node,6),('Motor',self.motor,4),
                ('Snelheid',self.speed,9),('Acceleratie',self.accel,9),('Deceleratie',self.decel,9),
                ('Pomptype',tests.fields['pump_type'],8),('Slaginstelling',tests.fields['stroke_setting'],12))):
            ttk.Label(setup,text=label).grid(row=0,column=col,sticky='w',padx=4)
            ttk.Entry(setup,textvariable=var,width=width).grid(row=1,column=col,padx=4)
        ttk.Button(setup,text='Huidige stand als startpositie 0',command=self.set_start_position).grid(row=2,column=0,columnspan=3,sticky='w',pady=6)
        ttk.Checkbutton(setup,text='MOVE_TO_DONE verplicht (optioneel)',variable=self.wait_done_var).grid(row=2,column=3,columnspan=4,sticky='w')
        self.counter_label=tk.StringVar()
        ttk.Label(setup,textvariable=self.counter_label,font=('Consolas',11)).grid(row=3,column=0,columnspan=6,sticky='w')
        ttk.Button(setup,text='STOP / Esc',command=self.abort_all).grid(row=3,column=6,sticky='e')
        ttk.Label(setup,text='Teller = volledig verzonden doelen; fysieke pompstand wordt niet gemeten. Prime omw bij kalibratie: 0 = overslaan.').grid(row=4,column=0,columnspan=7,sticky='w')
        panel.status.set('Meetcurve gebruikt huidige teller als beginpunt; geen HOME of extra ZERO bij start.')
        for child in char_page.winfo_children():
            if isinstance(child,ttk.Label) and 'Relatief: huidige stand' in str(child.cget('text')):
                child.configure(text='Curve begint bij huidige teller. Die beginpositie wordt opgeslagen.\n400 stappen per meetpunt; 5 herhalingen standaard.\nGrafiek toont uitstroom, geen interne plunjerverplaatsing.')
        self.build_results()
        notebook.select(notebook.tabs()[0])
        def feedback_mode_changed(*args):
            if not self.busy and panel.adapter is not None:
                panel.adapter.require_feedback=bool(self.wait_done_var.get())
        self.wait_done_var.trace_add('write',feedback_mode_changed)
        self.correction_enabled.trace_add('write',self.correction_changed)
        self.correction_percent.trace_add('write',self.correction_changed)
        self.root.after(200,self.refresh_counter)

    def get_dose_correction(self):
        if not self.correction_enabled.get():return 0.
        from curve_dosing import corrected_mass
        value=float(self.correction_percent.get().replace(',','.'))
        corrected_mass(1,value)
        return value

    def adjust_correction(self,delta):
        if self.busy:return
        try:
            value=float(self.correction_percent.get().replace(',','.'))+delta
            from curve_dosing import corrected_mass
            corrected_mass(1,value)
            self.correction_percent.set(f'{value:g}')
            self.correction_enabled.set(True)
        except Exception as exc:messagebox.showerror('Handmatige correctie',str(exc))

    def reset_correction(self):
        if self.busy:return
        self.correction_percent.set('0')
        self.correction_enabled.set(False)

    def correction_changed(self,*args):
        if self.busy:return
        self.preview_dose(silent=True)

    def build_results(self):
        self.measurement_rows=[]
        notebook=self.gravimetric_tests.page.master
        box=ttk.Frame(notebook,padding=6)
        self.results_page=box
        notebook.add(box,text='Meetresultaten')
        self.tree.configure(height=3)
        controls=ttk.Frame(box);controls.pack(fill='x')
        self.results_kind=tk.StringVar(value='Alle')
        ttk.Label(controls,text='Tonen').pack(side='left')
        selector=ttk.Combobox(controls,textvariable=self.results_kind,state='readonly',width=14,
                              values=('Alle','Kalibratie','360°','Dosering'))
        selector.pack(side='left',padx=6)
        selector.bind('<<ComboboxSelected>>',lambda event:self.render_results())
        ttk.Button(controls,text='Resultaten laden…',command=self.load_measurement_results).pack(side='left',padx=4)
        ttk.Button(controls,text='Overzicht als CSV',command=self.export_measurement_results).pack(side='left',padx=4)
        ttk.Button(controls,text='Open resultatenmap',command=self.open_measurement_folder).pack(side='left',padx=4)
        self.results_summary=tk.StringVar(value='Nog geen metingen. Resultaten verschijnen hier tijdens meetreeks.')
        ttk.Label(box,textvariable=self.results_summary,wraplength=1120).pack(anchor='w',pady=4)
        frame=ttk.Frame(box);frame.pack(fill='both',expand=True)
        self.result_columns=('kind','index','target_g','mass_g','error_g','error_pct','correction_pct','curve_target_g','cumulative_g','start_steps','end_steps','status')
        self.measurement_table=ttk.Treeview(frame,columns=self.result_columns,show='headings',height=6)
        titles=('Meting','Nr. / hoek','Gevraagd g','Gemeten g','Verschil g','Fout %','Correctie %','Curve-doel g','Cumulatief g','Teller van','Teller naar','Status')
        for key,title in zip(self.result_columns,titles):
            self.measurement_table.heading(key,text=title)
            self.measurement_table.column(key,width=100,minwidth=75)
        vertical=ttk.Scrollbar(frame,orient='vertical',command=self.measurement_table.yview)
        horizontal=ttk.Scrollbar(frame,orient='horizontal',command=self.measurement_table.xview)
        self.measurement_table.configure(yscrollcommand=vertical.set,xscrollcommand=horizontal.set)
        self.measurement_table.grid(row=0,column=0,sticky='nsew')
        vertical.grid(row=0,column=1,sticky='ns');horizontal.grid(row=1,column=0,sticky='ew')
        frame.rowconfigure(0,weight=1);frame.columnconfigure(0,weight=1)
        self.measurement_table.tag_configure('failed',foreground='#b82020')
        self.measurement_table.bind('<<TreeviewSelect>>',self.result_details)
        self.results_path=tk.StringVar(value='Schaalmetingen; tellerposities blijven softwarewaarden.')
        ttk.Label(box,textvariable=self.results_path,wraplength=1120).pack(anchor='w')

    def publish_measurement(self,kind,row,path=''):
        snapshot=normalize(kind,dict(row))
        snapshot['source']=str(path)
        self._ui(lambda r=snapshot:self.add_measurement_result(r))

    def add_measurement_result(self,row):
        self.measurement_rows.append(row)
        if row['kind']=='Dosering':
            if row['mass_g'] is None:
                self.dose_last_result.set('Geen geldige doseermeting: '+row.get('error','onvoltooid'))
            else:
                text=f"Gemeten {row['mass_g']:.3f} g"
                if row['target_g'] is not None:
                    text+=f" / gevraagd {row['target_g']:.3f} g · afwijking {row['error_g']:+.3f} g ({row['error_pct']:+.2f}%)" if row['error_pct'] is not None else f" / gevraagd {row['target_g']:.3f} g"
                if row.get('correction_pct') is not None:text+=f" · correctie {row['correction_pct']:+g}%"
                self.dose_last_result.set(('Onvoltooid · ' if row['status']=='failed' else '')+text)
        if row.get('source'):self.results_path.set('Bestand: '+row['source'])
        if self.results_kind.get() in ('Alle',row['kind']):self.insert_measurement_row(row)
        self.update_results_summary()

    def insert_measurement_row(self,row):
        def display(key):
            value=row.get(key)
            if key=='status':return {'completed':'gemeten','failed':'mislukt','running':'bezig'}.get(value,value)
            if value is None:return '—'
            return f'{value:.3f}' if key.endswith('_g') or key in ('error_pct','correction_pct') else str(value)
        item=self.measurement_table.insert('','end',values=[display(key) for key in self.result_columns],
                                           tags=('failed',) if row['status']=='failed' else ())
        self.measurement_table.see(item)

    def update_results_summary(self):
        rows=[r for r in self.measurement_rows if self.results_kind.get() in ('Alle',r['kind'])]
        measured=[r for r in rows if r['status']=='completed' and r['mass_g'] is not None]
        failed=sum(r['status']=='failed' for r in rows)
        text=f'{len(measured)} gemeten regels · {failed} mislukt'
        doses=[r for r in measured if r['kind']=='Dosering']
        if doses:
            requested=sum(r['target_g'] or 0 for r in doses)
            actual=sum(r['mass_g'] for r in doses)
            text+=f' | Doseringen: gevraagd {requested:.3f} g · gemeten {actual:.3f} g · verschil {actual-requested:+.3f} g'
            if len({r['target_g'] for r in doses})==1:
                import statistics
                sd=statistics.stdev(r['mass_g'] for r in doses) if len(doses)>1 else 0
                text+=f' | Gemiddeld {actual/len(doses):.3f} g · SD {sd:.3f} g'
        elif measured:
            last=measured[-1]
            text+=f" | Laatste {last['kind']}: {last['mass_g']:.3f} g"
            if last['cumulative_g'] is not None:text+=f" · cumulatief {last['cumulative_g']:.3f} g"
        self.results_summary.set(text)

    def render_results(self):
        self.measurement_table.delete(*self.measurement_table.get_children())
        for row in self.measurement_rows:
            if self.results_kind.get() in ('Alle',row['kind']):self.insert_measurement_row(row)
        self.update_results_summary()

    def result_details(self,event=None):
        items=self.measurement_table.selection()
        if not items:return
        visible=[r for r in self.measurement_rows if self.results_kind.get() in ('Alle',r['kind'])]
        index=self.measurement_table.index(items[0])
        if index<len(visible):
            row=visible[index]
            self.results_path.set(('Fout: '+row['error']+' | ' if row.get('error') else '')+'Bestand: '+row.get('source',''))

    def load_measurement_results(self):
        if self.busy:return
        path=filedialog.askopenfilename(title='Meetresultaten bekijken',filetypes=[('JSON-metingen','*.json')])
        if not path:return
        try:
            rows=list(records_from_payload(json.loads(Path(path).read_text(encoding='utf-8'))))
            for row in rows:row['source']=path
            self.measurement_rows=rows
            self.results_kind.set('Alle')
            self.render_results()
            self.results_path.set('Bekeken bestand: '+path+' (geen teller of curve geactiveerd)')
        except Exception as exc:messagebox.showerror('Resultaten laden',str(exc))

    def export_measurement_results(self):
        path=filedialog.asksaveasfilename(title='Meetoverzicht opslaan',defaultextension='.csv',
                                          filetypes=[('CSV','*.csv')],initialfile='meetresultaten.csv')
        if not path:return
        try:
            with Path(path).open('w',newline='',encoding='utf-8-sig') as stream:
                writer=csv.DictWriter(stream,fieldnames=list(self.result_columns)+['error','source'])
                writer.writeheader()
                writer.writerows(row for row in self.measurement_rows if self.results_kind.get() in ('Alle',row['kind']))
        except Exception as exc:messagebox.showerror('Resultaten opslaan',str(exc))

    def open_measurement_folder(self):
        import os
        directory=self.gravimetric_tests.directory
        directory.mkdir(parents=True,exist_ok=True)
        os.startfile(directory)

    def refresh_counter(self):
        try:
            cfg=self.gravimetric_tests.config()
            position=self.commanded[cfg.motor]
            valid=self.counter_valid and self.counter_bus is self.bus and self.counter_context==position_binding(cfg)
            adapter=self.gravimetric_tests.characterization.adapter
            if not self.busy and adapter is not None and not adapter.reference_valid:valid=False
            if not valid:self.counter_valid=False
            self.counter_label.set(f'Teller {position:,} stappen | fase vanaf teller-nul {position%cfg.steps_rev} / {cfg.steps_rev} | '+('software-referentie actief' if valid else 'startpositie kiezen'))
        except Exception:self.counter_label.set('Controleer pompinstellingen')
        if self.root.winfo_exists():self.root.after(250,self.refresh_counter)

    def workflow_config(self):
        fields=self.gravimetric_tests.fields
        keys=('pump_type','pump_id','stroke_setting','fluid','density','temperature_c',
              'density_source','scale_id','scale_resolution_g','tubing')
        return PumpConfig(node=self.node.get(),motor=self.motor.get(),steps_rev=self.fo_steps_rev.get(),
                          speed=self.speed.get(),accel=self.accel.get(),decel=self.decel.get(),
                          **{key:fields[key].get() for key in keys})

    def new_adapter(self):
        panel=self.gravimetric_tests.characterization
        if panel.adapter:panel.adapter.restore()
        panel.curve=panel.last_measurement=None
        panel.curve_status.set('Curve opnieuw meten na startpositie/kalibratie wijzigen.')
        adapter=CommandCounterAdapter(self,self.gravimetric_tests.config(),
                                      float(panel.values['position_tolerance'].get()))
        panel.adapter=adapter
        return adapter

    def run_minimal(self,label,operation):
        if self.busy:return
        if not self.bus:
            messagebox.showerror('Pomp','Eerst CAN verbinden');return
        self.fo_cancel.clear()
        self.busy='forward-only '+label
        self.gravimetric_tests.set_locked(True)
        def worker():
            error=''
            try:operation()
            except Exception as exc:
                error=str(exc)
                self.counter_valid=False
                adapter=self.gravimetric_tests.characterization.adapter
                if adapter:
                    try:adapter.abort()
                    except Exception as stop_error:error+='; stop: '+str(stop_error)
            self._ui(lambda e=error:finish(e))
        def finish(error):
            self.busy=None
            self.gravimetric_tests.set_locked(False)
            if error and not getattr(self,'closing_minimal',False):messagebox.showerror(label,error)
        threading.Thread(target=worker,daemon=True).start()

    def set_start_position(self):
        if self.busy:return
        try:adapter=self.new_adapter()
        except Exception as exc:messagebox.showerror('Startpositie',str(exc));return
        self.run_minimal('startpositie',adapter.establish_zero)

    def fo_start_calibrate(self):
        if self.busy:return
        if not self.scale:
            messagebox.showerror('Kalibratie','Eerst weegschaal verbinden');return
        try:
            count=integer(self.fo_anchor_n.get(),'Kalibratieherhalingen',100)
            prime=int(self.fo_prime_n.get())
            if not 0<=prime<=100:raise ValueError('Prime: 0–100 omwentelingen')
            adapter=self.new_adapter()
            adapter.set_relative_reference()
            measurement=Measurements(adapter,self.scale,self.scale_lock,
                        lambda k,d:self._event('CAL',None,b'',str(d)),
                        self.gravimetric_tests.directory,policy=self.measurement_policy())
        except Exception as exc:messagebox.showerror('Kalibratie',str(exc));return
        self.run_minimal('kalibratie',lambda:self.calibrate_forward(adapter,measurement,count,prime))

    def measurement_policy(self):
        fields=self.gravimetric_tests.fields
        return MeasurementPolicy(window_s=fields['window'].get(),range_g=fields['range'].get(),
                                 timeout_s=fields['timeout'].get(),settle_min_s=fields['settle'].get())

    def calibrate_forward(self,adapter,measurement,count,prime=0):
        import statistics
        records=[]
        base=self.gravimetric_tests.directory/('forward_cal_'+datetime.now().strftime('%Y%m%d_%H%M%S_%f'))
        base.parent.mkdir(parents=True,exist_ok=True)
        payload=dict(status='running',context=adapter.cfg.context(),records=records,scale_samples=measurement.scale_samples,
                     start_position_steps=adapter.position,physical_position_verified=False)
        def save():
            atomic_text(base.with_suffix('.json'),json.dumps(payload,indent=2))
            stream=io.StringIO(newline='')
            writer=csv.DictWriter(stream,fieldnames=sorted({key for row in records for key in row}))
            writer.writeheader();writer.writerows(records)
            atomic_text(base.with_suffix('.csv'),stream.getvalue())
        try:
            if prime:adapter.move_absolute(adapter.position+prime*adapter.cfg.steps_rev)
            for repeat in range(1,count+1):
                start=adapter.position
                row=dict(repeat=repeat,status='running',start_steps=start,target_steps=start+adapter.cfg.steps_rev)
                records.append(row)
                try:
                    before=measurement.stable_weight()
                    adapter.move_absolute(start+adapter.cfg.steps_rev)
                    adapter.sleep(measurement.policy.settle_min_s)
                    after=measurement.stable_weight()
                    row.update(status='completed',before_g=before,after_g=after,mass_g=after-before,
                               feedback=adapter.last_motion_feedback)
                    self._ui(lambda r=dict(row):self._fo_row(dict(index=r['repeat'],kind='kalibratie',
                        target_g=None,dispensed_g=r['mass_g'],error_pct=None,steps=adapter.cfg.steps_rev,
                        gpr=r['mass_g'],duration_s=None)))
                except Exception as exc:row.update(status='failed',error=str(exc));raise
                finally:
                    row['end_position_steps']=adapter.position
                    save()
                    self.publish_measurement('Kalibratie',row,base.with_suffix('.json'))
            values=[r['mass_g'] for r in records]
            mean=statistics.mean(values)
            if mean<=0:raise ValueError('Geen positieve opbrengst gemeten')
            payload.update(status='completed',g_per_rev=mean,sd_g=statistics.stdev(values) if len(values)>1 else 0)
            self.fo_cal=payload
            self._ui(lambda:(self.fo_gpr.set(f'{mean:.6f}'),self.fo_cal_lbl.configure(text=f'{mean:.6f} g/omw bij huidige slag; teller loopt door'),self.fo_status_lbl.configure(text='Kalibratie klaar. Meet nu 360°-curve.')))
        except Exception as exc:payload.update(status='failed',error=str(exc));raise
        finally:
            payload['end_position_steps']=adapter.position
            save()

    def fo_load_cal(self):
        if self.busy:return
        path=filedialog.askopenfilename(title='Forward-only kalibratie laden',filetypes=[('Kalibratie','*.json')])
        if not path:return
        try:
            from ceramic_pump import positive
            payload=json.loads(Path(path).read_text(encoding='utf-8'))
            mass=positive(payload.get('g_per_rev',payload.get('g_per_pump_rev')),'g/omwenteling')
            self.fo_cal=payload
            self.fo_gpr.set(f'{mass:.6f}')
            self.fo_cal_lbl.configure(text=f'Geladen: {mass:.6f} g/omw. Meetcurve en startreferentie apart vastleggen.')
        except Exception as exc:messagebox.showerror('Kalibratie laden',str(exc))

    def preview_dose(self,silent=False):
        panel=self.gravimetric_tests.characterization
        try:
            if not panel.curve or not panel.last_measurement:raise ValueError('Eerst meetcurve afronden')
            panel.adapter.check_reference()
            from ceramic_pump import positive
            target_g=positive(panel.curve_target.get(),'Doelmassa')
            origin=panel.last_measurement.payload['reference_origin_steps']
            start=panel.adapter.position
            from curve_dosing import corrected_mass
            correction_pct=self.get_dose_correction()
            curve_grams=corrected_mass(target_g,correction_pct)
            target=origin+panel.curve.target(start-origin,curve_grams)
            self.dose_preview.set(f'{target_g:g} g · correctie {correction_pct:+g}% → curve-doel {curve_grams:g} g: teller {start} → {target}; verplaatsing {target-start} stappen; curvefase {(start-origin)%panel.curve.steps_rev} → {(target-origin)%panel.curve.steps_rev}.')
        except Exception as exc:
            if silent:self.dose_preview.set('Berekening: '+str(exc))
            else:messagebox.showerror('Doseerberekening',str(exc))

    def abort_all(self):
        self.counter_valid=False
        panel=self.gravimetric_tests.characterization
        if self.busy and panel.adapter:
            self.fo_cancel.set()
            threading.Thread(target=panel.adapter.abort,daemon=True).start()

    def disconnect(self):
        self.counter_valid=False
        super().disconnect()

    def disconnect_scale(self):
        self.counter_valid=False
        super().disconnect_scale()

    def on_close(self):
        if getattr(self,'closing_minimal',False):return
        self.closing_minimal=True
        self.abort_all()
        self.root.title('Pump Debug — stoppen en afsluiten…')
        def finish_close():
            if self.busy:
                self.root.after(100,finish_close)
                return
            if self.scale:self.disconnect_scale()
            if self.bus:self.disconnect()
            self.root.destroy()
        self.root.after(100,finish_close)


def main():
    root=tk.Tk()
    try:ttk.Style().theme_use('vista')
    except tk.TclError:pass
    PumpDebugApp(root)
    root.mainloop()


if __name__=='__main__':main()
