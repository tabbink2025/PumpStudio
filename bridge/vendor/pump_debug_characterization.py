"""360-degree test plugged into the original GUI without replacing its methods."""
import csv
import io
import json
from datetime import datetime
from uuid import uuid4
import threading
import time
from types import SimpleNamespace
import tkinter as tk
from tkinter import ttk, messagebox
from characterization import Characterization, angle_targets, step_targets, plot_characterization
from ceramic_pump import Cancelled, positive, integer
from pump_debug_tests import LegacyPumpAdapter


class CharacterizationAdapter(LegacyPumpAdapter):
    def __init__(self, app, config, position_tolerance=64):
        super().__init__(app, config, bool(config.priming_confirmed))
        value = float(position_tolerance)
        if not value.is_integer() or not 0 <= value <= 10000:
            raise ValueError('Tellertolerantie moet een geheel getal van 0 tot 10000 zijn')
        self.position_tolerance = int(value)
        self.last_motion_feedback = None
        self.retained = False
        self.homed = False
        self.reference_valid = False
        self.reference_mode = 'home'
        self.homing_status = 'not_homed'
        self.responses = {}
        self.condition = threading.Condition()
        self.fault = None
        self.local = threading.local()
        self.original_feedback = app._track_feedback
        self.original_send = app._send_frame
        app._track_feedback = self.feedback
        app._send_frame = self.send_frame

    def restore(self):
        self.app._track_feedback = self.original_feedback
        self.app._send_frame = self.original_send

    def feedback(self, node, data):
        if node == self.cfg.node and len(data) == 8 and data[2] == self.cfg.motor and data[3] == 0xAA:
            value = int.from_bytes(data[4:], 'little', signed=True)
            with self.condition:
                self.responses[(data[0], data[1])] = (time.monotonic(), value)
                if data[0] == 0 and data[1] == 4 and not value:
                    self.homed = False
                    self.homing_status = 'reported_not_homed'
                    if self.reference_mode == 'home':
                        self.reference_valid = False
                if (0x81 <= data[1] <= 0x88 and value) or (data[0] == 0xFF and data[1] == 1):
                    self.homed = False
                    self.reference_valid = False
                    self.fault = 'Motorfout tijdens characterization'
                self.condition.notify_all()
        self.original_feedback(node, data)

    def send_frame(self, node, data):
        if self.retained and not getattr(self.local, 'curve_motion', False) and not getattr(self.local, 'emergency', False):
            # Preserve all original controls. Any outside execute command on
            # this motor invalidates the retained reference, including ZERO.
            if node == self.cfg.node and len(data) == 8 and data[2] == self.cfg.motor and data[1] == 1 and data[0] not in (0, 71):
                self.reference_valid = False
            return self.original_send(node, data)
        if not getattr(self.local, 'emergency', False):
            self.check()
        return self.original_send(node, data)

    def check(self):
        try:
            super().check()
        except Exception:
            self.reference_valid = False
            raise
        if self.fault:
            raise RuntimeError(self.fault)

    def check_homed(self):
        self.check()
        if not self.homed:
            raise RuntimeError('Homingreferentie verloren')

    def check_reference(self):
        self.check()
        if self.reference_mode == 'home':
            self.check_homed()
        if not self.reference_valid or self.position is None:
            raise RuntimeError('Positiereferentie verloren; opnieuw refereren vereist')

    def set_relative_reference(self):
        from pump_debug import COMMANDS
        self.check()
        self.reference_mode = 'relative'
        self.reference_valid = False
        self.homed = False
        self.homing_status = 'relative_zero_pending'
        since = time.monotonic()
        if not self.app._sequence(self.cfg.node, self.cfg.motor, COMMANDS['ZERO'], 0, 0, 0, self.cfg.decel):
            raise RuntimeError('Relatieve ZERO verzenden mislukt')
        self.wait(76, since, 3, 0)
        self.app.commanded[self.cfg.motor] = 0
        self.reference_valid = True
        self.homing_status = 'relative_ZERO_confirmed_no_physical_home'

    def wait(self, code, since, timeout, expected=None, tolerance=0):
        deadline = time.monotonic() + timeout
        with self.condition:
            while True:
                self.check()
                response = self.responses.get((code, 1))
                if response and response[0] >= since:
                    if code == 74 and expected is not None:
                        self.last_motion_feedback = dict(target_position_steps=expected,
                            reported_position_steps=response[1], controller_position_error_steps=response[1]-expected,
                            position_tolerance_steps=tolerance,
                            position_basis='commanded_steps_with_controller_counter_check',
                            physical_position_verified=False)
                    if expected is not None and abs(response[1]-expected) > tolerance:
                        raise RuntimeError(f'Controller-teller wijkt af: doel {expected}, gemeld {response[1]}, verschil {response[1]-expected:+d}; tolerantie +/-{tolerance} stappen. Geen fysieke positiemeting.')
                    return response[1]
                if time.monotonic() >= deadline:
                    raise TimeoutError(f'Geen verse bevestiging voor code {code}')
                self.condition.wait(.05)

    def home(self, timeout=60):
        # Original HOME and ZERO encoder, no INIT/START substituted into old UI.
        from pump_debug import COMMANDS
        self.homed = False
        self.reference_mode = 'home'
        self.reference_valid = False
        self.homing_status = 'homing'
        for name, response, expected in (('HOME', 77, None), ('ZERO', 76, 0)):
            self.check()
            since = time.monotonic()
            if not self.app._sequence(self.cfg.node, self.cfg.motor, COMMANDS[name], 0,
                                      self.cfg.speed, self.cfg.accel, self.cfg.decel):
                raise RuntimeError(f'{name} verzenden mislukt')
            self.wait(response, since, timeout if name == 'HOME' else 3, expected)
        self.app.commanded[self.cfg.motor] = 0
        self.homed = True
        self.homing_status = 'HOME_DONE_then_ZERO_confirmed'
        self.reference_valid = True

    def move_absolute(self, target):
        self.check_reference()
        if target <= self.position:
            raise ValueError('Uitsluitend vooruit')
        since = time.monotonic()
        self.last_motion_feedback = None
        self.local.curve_motion = True
        try:
            super().move(target-self.position)
            # Fresh controller counter feedback, not encoder/pump-angle proof.
            self.wait(74, since, 1, target, tolerance=self.position_tolerance)
            self.check_reference()
            if self.last_motion_feedback:
                getattr(self.app, '_event', lambda *args: None)('POSITION_COUNTER', None, b'', str(self.last_motion_feedback))
        except Exception:
            self.reference_valid = False
            raise
        finally:
            self.local.curve_motion = False

    def abort(self):
        from pump_debug import COMMANDS
        self.cancel.set()
        self.homed = False
        self.reference_valid = False
        self.homing_status = 'stopped_reference_invalid'
        self.local.emergency = True
        try:
            if not self.app._sequence(self.cfg.node, self.cfg.motor, COMMANDS['INIT'], 0, 0, 0, 0):
                raise RuntimeError('Stop niet verzonden; controleer fysieke noodstop')
        finally:
            self.local.emergency = False


class CharacterizationPanel:
    def __init__(self, tests, notebook):
        self.tests, self.app = tests, tests.app
        self.busy = False
        self.adapter = None
        self.last_measurement = self.curve = None
        self.curve_bus = self.curve_scale = None
        page = ttk.Frame(notebook, padding=8)
        notebook.add(page, text='360° Characterization')
        self.values = {}
        for row, (key, label, default) in enumerate((('interval','Meetinterval',400),('repeats','Herhalingen',5),('settle','Uitlooptijd (s)',2),('home_timeout','Homingtimeout (alleen sensor, s)',60),('position_tolerance','Tellertolerantie (stappen; geen encoder)',64))):
            ttk.Label(page, text=label).grid(row=row,column=0,sticky='w')
            self.values[key] = tk.StringVar(value=str(default))
            ttk.Entry(page,textvariable=self.values[key]).grid(row=row,column=1)
        self.reference_choice = tk.StringVar(value='Relatieve startpositie (zonder homesensor)')
        self.interval_unit = tk.StringVar(value='Motorstappen')
        ttk.Label(page, text='Referentie').grid(row=5,column=0,sticky='w')
        ttk.Combobox(page,textvariable=self.reference_choice,state='readonly',width=42,
                     values=('Relatieve startpositie (zonder homesensor)', 'Homesensor')).grid(row=5,column=1,sticky='w')
        ttk.Label(page,text='Intervaleenheid').grid(row=6,column=0,sticky='w')
        ttk.Combobox(page,textvariable=self.interval_unit,state='readonly',values=('Motorstappen','Graden')).grid(row=6,column=1,sticky='w')
        ttk.Button(page,text='360° Characterization starten',command=self.start).grid(row=7,column=0)
        ttk.Button(page,text='STOP characterization',command=self.stop).grid(row=7,column=1)
        self.status=tk.StringVar(value='CAN en schaal verbinden; pomp vooraf bedrijfsklaar en geprimed houden. Geen INIT+START-voorwaarde. Context uit Gravimetrische tests.')
        ttk.Label(page,textvariable=self.status,wraplength=1000).grid(row=8,column=0,columnspan=4,sticky='w')
        ttk.Label(page,text='Relatief: huidige stand → bevestigde teller-ZERO → tarra → meten. Geen HOME.\nSensorstand: HOME → ZERO → tarra → meten.\nGemeten uitstroom; geen directe interne plunjerverplaatsing.\nPomp, stappen/omw en meetbeleid instellen bij Gravimetrische tests.').grid(row=9,column=0,columnspan=4,sticky='w')
        curve_box = ttk.LabelFrame(page, text='Direct doseren met gemeten curve', padding=6)
        curve_box.grid(row=10,column=0,columnspan=4,sticky='ew',pady=8)
        self.curve_target=tk.StringVar(value='5')
        self.curve_repeats=tk.StringVar(value='2')
        ttk.Label(curve_box,text='Doel (g)').grid(row=0,column=0)
        ttk.Entry(curve_box,textvariable=self.curve_target,width=12).grid(row=0,column=1)
        ttk.Label(curve_box,text='Herhalingen').grid(row=0,column=2)
        ttk.Entry(curve_box,textvariable=self.curve_repeats,width=6).grid(row=0,column=3)
        ttk.Button(curve_box,text='Doseer vanuit curve — geen prime/trim',command=self.start_curve_dose).grid(row=1,column=0,columnspan=4,sticky='ew')
        self.curve_status=tk.StringVar(value='Eerst characterization afronden; positie behouden.')
        ttk.Label(curve_box,textvariable=self.curve_status,wraplength=900).grid(row=2,column=0,columnspan=4,sticky='w')
        self.last_type='INV-13' 
        self.steps_by_type={'INV-13':'25600','INV-36':tests.fields['steps_rev'].get()}
        tests.fields['steps_rev'].set('25600')
        def select(*args):
            self.steps_by_type[self.last_type]=tests.fields['steps_rev'].get()
            self.last_type=tests.fields['pump_type'].get()
            tests.fields['steps_rev'].set(self.steps_by_type[self.last_type])
        tests.fields['pump_type'].trace_add('write',select)

    def stop(self):
        if self.adapter and self.busy:
            self.app.fo_cancel.set()
            # Interrupt immediately; original encoder's shared send_lock keeps
            # INIT after the cancelled sequence. Per-frame guard blocks MOVE.
            def work():
                try:
                    self.adapter.abort()
                except Exception as exc:
                    self.app._event('ERR',None,b'',str(exc))
            threading.Thread(target=work,daemon=True).start()

    def start(self):
        if self.app.busy or self.busy:
            return
        adapter=None
        try:
            if not self.app.bus or not self.app.scale:
                raise ValueError('Eerst CAN en weegschaal verbinden')
            interval=positive(self.values['interval'].get(),'Hoekinterval')
            repeats=integer(self.values['repeats'].get(),'Herhalingen',100)
            settle=float(self.values['settle'].get())
            home_timeout=positive(self.values['home_timeout'].get(),'Homingtimeout')
            config=self.tests.config()
            interval_steps = integer(self.values['interval'].get(), 'Meetinterval', config.steps_rev) if self.interval_unit.get() == 'Motorstappen' else None
            (step_targets(config.steps_rev,interval_steps) if interval_steps is not None else angle_targets(config.steps_rev,interval))
            reference_mode = 'relative' if self.reference_choice.get().startswith('Relatieve') else 'home'
            from matplotlib.backends.backend_tkagg import FigureCanvasTkAgg
            from ceramic_pump import MeasurementPolicy
            policy=MeasurementPolicy(window_s=self.tests.fields['window'].get(), range_g=self.tests.fields['range'].get(),timeout_s=self.tests.fields['timeout'].get(),settle_min_s=settle)
            self.app.fo_cancel.clear()
            if self.adapter:
                self.adapter.restore()
            self.last_measurement = self.curve = None
            self.curve_status.set('Nieuwe characterization loopt; vorige curve niet actief.')
            tolerance=float(self.values['position_tolerance'].get())
            adapter=CharacterizationAdapter(self.app,config,position_tolerance=tolerance)
            def tare():
                adapter.check_reference()
                with self.app.scale_lock:
                    return self.app.scale.tare()
            scale=SimpleNamespace(get_reading=self.app.read_scale,tare=tare)
            m=Characterization(adapter,scale,threading.Lock(),lambda *args: None,self.tests.directory,policy=policy)
            m.result_sink=lambda row:getattr(self.app,'publish_measurement',lambda *args:None)('360°',row,m.base.with_suffix('.json'))
            m.test_metadata={'scale': {'id':config.context().get('scale_id')},'source':'original pump_debug GUI', 'init_start_required':False, 'preparation_confirmed':bool(config.priming_confirmed), 'measurement_policy':policy.context(), 'position_tolerance_steps':adapter.position_tolerance, 'physical_position_verified':False, 'position_basis':'sent_absolute_targets' if not adapter.require_feedback else 'controller_counter_checked', 'move_done_required':adapter.require_feedback}
        except Exception as exc:
            if adapter:
                adapter.restore()
            messagebox.showerror('Characterization',str(exc))
            return
        self.adapter=adapter
        self.busy=True
        self.app.busy='forward-only 360 characterization'
        self.tests.set_locked(True)
        self.status.set('Characterization loopt; relatieve ZERO, tarra, meten…' if reference_mode == 'relative' else 'Characterization loopt; HOME, tarra, meten…')
        def worker():
            error=''
            try:
                m.run(interval,repeats,settle,home_timeout,reference_mode=reference_mode,interval_steps=interval_steps)
            except Exception as exc:
                error=str(exc)
            finally:
                if error or not hasattr(m, 'payload'):
                    adapter.restore()
                else:
                    adapter.retained = True
                self.app._ui(lambda e=error: finish(e))
        def finish(error):
            self.busy=False
            self.app.busy=None
            self.tests.set_locked(False)
            self.status.set(('Gestopt: '+error) if error else 'Afgerond: '+str(m.base))
            if not error and hasattr(m, 'payload'):
                self.last_measurement = m
                self.curve_bus, self.curve_scale = self.app.bus, self.app.scale
                self.curve_expected_position = adapter.position
                try:
                    from curve_dosing import OutflowCurve
                    self.curve = OutflowCurve(m.payload, max_correction_g=m.policy.range_g, allow_smoothing=True)
                    m.payload['dose_curve']=self.curve.context()
                    m.save()
                    self.curve_status.set(f'Curve actief: {self.curve.g_per_rev:.5f} g/omw. Teller {adapter.position}. '+(f'Meetruis gladgestreken: max {self.curve.correction_g:.5f} g; proefmodel, controleer doses.' if self.curve.quality_warning else 'Direct doseren zonder prime.'))
                except Exception as exc:
                    self.curve_status.set('Curve niet bruikbaar voor doseren: '+str(exc))
            if hasattr(m,'payload'):
                figure=plot_characterization(m.payload)
                figure.savefig(str(m.base)+'.png')
                window=tk.Toplevel(self.app.root)
                window.title('360° Characterization — '+m.payload['status'])
                canvas=FigureCanvasTkAgg(figure,master=window)
                canvas.draw()
                canvas.get_tk_widget().pack(fill='both',expand=True)
                window._chart=canvas
            if error and not getattr(self.app, 'closing_minimal', False):
                messagebox.showerror('Characterization',error)
        threading.Thread(target=worker,daemon=True).start()

    def start_curve_dose(self):
        if self.busy or self.app.busy:
            return
        try:
            if not self.curve or not self.last_measurement or not self.adapter:
                raise ValueError('Eerst een bruikbare characterization afronden')
            if self.app.bus is not self.curve_bus or self.app.scale is not self.curve_scale:
                raise ValueError('Verbinding gewijzigd; opnieuw characterization uitvoeren')
            self.adapter.check_reference()
            if self.adapter.position != self.curve_expected_position:
                raise ValueError('Positie gewijzigd; opnieuw characterization uitvoeren')
            context = self.tests.config().context()
            expected = self.adapter.cfg.context()
            if any(context.get(k) != v for k, v in expected.items() if k != 'priming_confirmed'):
                raise ValueError('Pomp- of motorinstellingen gewijzigd; opnieuw characterization uitvoeren')
            grams = positive(self.curve_target.get(), 'Doelmassa')
            repeats = integer(self.curve_repeats.get(), 'Herhalingen', 100)
            from curve_dosing import corrected_mass
            correction_pct=getattr(self.app,'get_dose_correction',lambda:0)()
            curve_grams=corrected_mass(grams,correction_pct)
            origin = self.last_measurement.payload['reference_origin_steps']
            # Validate whole requested series before the first hardware move.
            preview_position = self.adapter.position-origin
            for _ in range(repeats):
                preview_position = self.curve.target(preview_position, curve_grams)
        except Exception as exc:
            messagebox.showerror('Doseren vanuit curve',str(exc))
            return
        self.busy=True
        self.app.busy='forward-only doseren vanuit curve'
        self.tests.set_locked(True)
        self.status.set('Doseren vanuit gemeten curve; geen prime, nulstelling of trim.')
        threading.Thread(target=self._curve_dose_worker,args=(grams,repeats,origin,correction_pct),daemon=True).start()

    def _curve_dose_worker(self, grams, repeats, origin, correction_pct=0):
        from ceramic_pump import atomic_text
        from curve_dosing import corrected_mass
        curve_grams=corrected_mass(grams,correction_pct)
        m, adapter = self.last_measurement, self.adapter
        base = self.tests.directory / ('curve_dose_'+datetime.now().strftime('%Y%m%d_%H%M%S_')+uuid4().hex[:8])
        data = dict(status='running', source_characterization=str(m.base),
                    started=datetime.now().astimezone().isoformat(), context=adapter.cfg.context(),
                    curve=self.curve.context(), reference_origin_steps=origin,
                    reference_mode=adapter.reference_mode, primed_again=False, trim=False,
                    target_g=grams,curve_target_g=curve_grams,manual_correction_pct=correction_pct,
                    correction_factor=1+correction_pct/100,repeats=repeats,records=[],scale_samples=[])
        sample_start=len(m.scale_samples)
        def save():
            data['scale_samples']=m.scale_samples[sample_start:]
            atomic_text(base.with_suffix('.json'),json.dumps(data,indent=2,ensure_ascii=False))
            rows=data['records']
            stream=io.StringIO(newline='')
            writer=csv.DictWriter(stream,fieldnames=sorted({k for r in rows for k in r}))
            writer.writeheader();writer.writerows(rows)
            atomic_text(base.with_suffix('.csv'),stream.getvalue())
        error=''
        try:
            save()
            for repeat in range(1,repeats+1):
                adapter.check_reference()
                if adapter.position != self.curve_expected_position:
                    raise RuntimeError('Positie onverwacht gewijzigd')
                start=adapter.position
                target=origin+self.curve.target(start-origin,curve_grams)
                row=dict(repeat=repeat,status='running',target_g=grams,curve_target_g=curve_grams,
                         manual_correction_pct=correction_pct,correction_factor=1+correction_pct/100,start_position_steps=start,
                         target_position_steps=target,steps=target-start,
                         start_phase_steps=(start-origin)%self.curve.steps_rev,
                         end_phase_steps=(target-origin)%self.curve.steps_rev,
                         predicted_g=self.curve.cumulative(target-origin)-self.curve.cumulative(start-origin))
                data['records'].append(row)
                save()
                try:
                    m.active_id=f'{base.name}-{repeat}'
                    m.stage='curve_dose_before'
                    before=m.stable_weight()
                    adapter.check_reference()
                    adapter.move_absolute(target)
                    adapter.sleep(m.payload['settle_s'])
                    m.stage='curve_dose_after'
                    after=m.stable_weight()
                    adapter.check_reference()
                    row.update(status='completed',before_g=before,after_g=after,
                               mass_g=after-before,error_g=after-before-grams,
                               error_pct=100*(after-before-grams)/grams)
                    self.curve_expected_position=adapter.position
                except Exception as exc:
                    row.update(status='failed',error=str(exc))
                    raise
                finally:
                    row.update(end_position_steps=adapter.position,reference_valid=adapter.reference_valid)
                    if getattr(adapter, 'last_motion_feedback', None):
                        row.update(adapter.last_motion_feedback)
                    save()
                    getattr(self.app,'publish_measurement',lambda *args:None)('Dosering',dict(row),base.with_suffix('.json'))
            data['status']='completed'
        except Exception as exc:
            error=str(exc)
            data.update(status='failed',error=error)
            try:
                adapter.abort()
            except Exception as stop_error:
                data['stop_error']=str(stop_error)
        finally:
            data.update(ended=datetime.now().astimezone().isoformat(),
                        final_position_steps=adapter.position,reference_valid=adapter.reference_valid)
            try:
                save()
            except Exception as exc:
                error=error or str(exc)
            self.app._ui(lambda e=error: self._curve_finished(e,base,data))

    def _curve_finished(self,error,base,data):
        self.busy=False
        self.app.busy=None
        self.tests.set_locked(False)
        if error:
            self.curve_status.set('Curve/referentie ongeldig na fout; opnieuw characterization uitvoeren.')
            self.status.set('Doseren afgebroken: '+error)
            if not getattr(self.app, 'closing_minimal', False):
                messagebox.showerror('Doseren vanuit curve',error)
        else:
            last=data['records'][-1]
            self.status.set(f"Curve-dosering klaar: laatste dosis {last['mass_g']:.5f} g; positie {last['end_position_steps']}. Resultaten: {base}")
            self.curve_status.set(f'Curve blijft actief vanaf {self.adapter.position} stappen; geen prime nodig zolang referentie en vulling behouden blijven.')
