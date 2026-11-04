"""Gravimetric tests added to the original pump_debug GUI.

No CAN connection, frame encoding, initialization or stop implementation here.
All hardware access goes through the original app's existing methods.
"""
import json
import threading
import time
from pathlib import Path
from types import SimpleNamespace
import tkinter as tk
from tkinter import ttk, messagebox, filedialog

from ceramic_pump import (Measurements, MeasurementPolicy, PumpConfig, Cancelled,
                          positive, integer, load_results, save_profile, validate_profile)


class LegacyPumpAdapter:
    """Reuse the original motion path, including its feedback and stop rules."""
    def __init__(self, app, config, priming_confirmed):
        self.app, self.cfg = app, config
        self.cancel = app.fo_cancel
        self.primed = bool(priming_confirmed)
        self.ready = bool(app.bus)

    @property
    def position(self):
        return self.app.commanded[self.cfg.motor]

    def check(self):
        if self.cancel.is_set():
            raise Cancelled("Test geannuleerd")
        if not self.app.bus:
            raise RuntimeError("CAN-verbinding gesloten")

    def sleep(self, seconds):
        if self.cancel.wait(seconds):
            self.check()
        self.check()

    def move(self, steps):
        self.check()
        steps = integer(steps, "Stappen")
        target = self.position + steps
        if target > 2147483647:
            raise ValueError("Positieteller vol")
        self.app._fo_move_forward(self.cfg.node, self.cfg.motor, target,
                                  self.cfg.speed, self.cfg.accel, self.cfg.decel)
        self.check()


class LegacyTestPanel:
    def __init__(self, app, notebook):
        self.app = app
        self.busy = self.stopping = False
        self.worker = None
        self.locked_widgets = []
        self.measurement = self.profile = None
        self.display_records, self.display_samples, self.display_summary = [], [], []
        self.directory = Path(__file__).resolve().parents[1] / "logs" / "pump_debug" / "ceramic_tests"
        self.page = ttk.Frame(notebook)
        notebook.add(self.page, text="Gravimetrische tests")
        self.progress = tk.StringVar(value="Pomp bedrijfsklaar en geprimed houden; verbindingen via oorspronkelijke bediening.")
        self.summary_text = tk.StringVar(value="Nog geen meetresultaten")
        self.graph_hint = tk.StringVar(value="Geen lineaire fit binnen één omwenteling")
        self.graph_mode = tk.StringVar(value="Massa / omwentelingen")
        self.confirmed = tk.BooleanVar(value=False)
        self.fields = {}
        self.buttons = []
        canvas = tk.Canvas(self.page, highlightthickness=0, height=480)
        scroll = ttk.Scrollbar(self.page, orient="vertical", command=canvas.yview)
        scroll.pack(side="right", fill="y")
        canvas.pack(side="left", fill="both", expand=True)
        canvas.configure(yscrollcommand=scroll.set)
        content = ttk.Frame(canvas, padding=8)
        canvas.create_window((0, 0), window=content, anchor="nw")
        content.bind("<Configure>", lambda event: canvas.configure(scrollregion=canvas.bbox("all")))
        ttk.Label(content, text="Tests gebruiken bestaande node, motor, snelheid, CAN-bediening en weegschaal. Geen automatische initialisatie.", wraplength=1050).grid(row=0, column=0, columnspan=4, sticky="w")
        definitions = [
            ("pump_type", "Pomptype", "INV-13"), ("pump_id", "Pomp-ID", ""),
            ("stroke_setting", "Mechanische slaginstelling", ""),
            ("steps_rev", "Stappen / werkelijke pompomw", app.fo_steps_rev.get()),
            ("fluid", "Vloeistof", app.fo_fluid.get()), ("density", "Dichtheid g/mL", ""),
            ("temperature_c", "Temperatuur °C", ""), ("density_source", "Herkomst dichtheid", ""),
            ("scale_id", "Schaal-ID", ""), ("scale_resolution_g", "Schaalresolutie g", ""),
            ("tubing", "Slangconfiguratie", ""), ("counts", "Omwentelingen (; scheiding)", "0.125; 0.25; 0.5; 1; 2; 5"),
            ("repeats", "Herhalingen", "5"), ("rpms", "RPM (; leeg = huidige snelheid)", ""),
            ("phases", "Startfasen deeldoses (0–<1)", "0; 0.25; 0.5; 0.75"),
            ("reference", "Referentieomw (leeg = geen)", "1"), ("blank", "Blancoduur s (leeg = geen)", "5"),
            ("window", "Stabiliteitsvenster s", "1.2"), ("range", "Max. massavariatie g", "0.002"),
            ("timeout", "Weegtimeout s", "30"), ("settle", "Minimale uitlooptijd s", "2"),
            ("cal_rev", "Kalibratie: gehele omw/meting", "20"), ("cal_repeats", "Kalibratie: metingen", "3")]
        for index, (key, label, default) in enumerate(definitions):
            row, col = index//2+1, index%2*2
            ttk.Label(content, text=label).grid(row=row, column=col, sticky="w", padx=4, pady=2)
            var = tk.StringVar(value=default)
            self.fields[key] = var
            if key == "pump_type":
                widget = ttk.Combobox(content, textvariable=var, values=("INV-13", "INV-36"), state="readonly", width=25)
            else:
                widget = ttk.Entry(content, textvariable=var, width=28)
            widget.grid(row=row, column=col+1, sticky="ew", padx=4, pady=2)
        row = (len(definitions)+1)//2+1
        ttk.Checkbutton(content, text="Controller bedrijfsklaar, pomp geprimed en opvang gereed", variable=self.confirmed).grid(row=row, column=0, columnspan=4, sticky="w")
        actions = ttk.Frame(content)
        actions.grid(row=row+1, column=0, columnspan=4, sticky="ew", pady=6)
        for title, action in (("Preview", self.preview), ("Testmatrix starten", lambda: self.start("protocol")),
                              ("Blanco zonder beweging", lambda: self.start("blank")),
                              ("Kalibratie meten", lambda: self.start("calibration")),
                              ("Gebruik gemeten g/omw", self.apply_calibration),
                              ("Resultaten laden", self.open_results)):
            button = ttk.Button(actions, text=title, command=action)
            button.pack(side="left", padx=3)
            self.buttons.append(button)
        ttk.Button(actions, text="Stop (oorspronkelijk)", command=app.cancel_forward_only).pack(side="left", padx=3)
        ttk.Label(content, textvariable=self.progress, wraplength=1050).grid(row=row+2, column=0, columnspan=4, sticky="w", pady=5)
        self.progress_bar = ttk.Progressbar(content, maximum=100)
        self.progress_bar.grid(row=row+3, column=0, columnspan=4, sticky="ew")
        plan_cols = ("kind", "rev", "rpm", "phase", "repeat", "shift")
        self.plan_table = self.table(content, row+4, plan_cols, ("Test", "Omw", "RPM", "Fase", "Herhaling", "Faseverplaatsing stappen"), 4)
        self.results_table = self.table(content, row+5, ("kind", "rev", "phase", "mass", "volume", "status", "seconds"),
                                        ("Test", "Omw", "Startfase", "Massa g", "Volume mL", "Status", "Tijd s"), 5)
        ttk.Label(content, textvariable=self.summary_text, wraplength=1050).grid(row=row+6, column=0, columnspan=4, sticky="w")
        self.summary_table = self.table(content, row+7, ("kind", "rev", "rpm", "phase", "n", "mean", "sd", "cv"),
                                       ("Test", "Omw", "RPM", "Fase", "n", "Gem. g", "SD g", "CV %"), 4)
        graph_choice = ttk.Combobox(content, textvariable=self.graph_mode, state="readonly", width=30,
                                    values=("Massa / omwentelingen", "Massa / startfase", "CV / testgroep", "Gewicht / tijd"))
        graph_choice.grid(row=row+8, column=0, columnspan=2, sticky="w", pady=4)
        graph_choice.bind("<<ComboboxSelected>>", lambda event: self.draw_graph())
        self.graph = tk.Canvas(content, width=1040, height=230, background="white", highlightthickness=1)
        self.graph.grid(row=row+9, column=0, columnspan=4, sticky="ew")
        self.graph.bind("<Configure>", lambda event: self.draw_graph())
        ttk.Label(content, textvariable=self.graph_hint, wraplength=1050).grid(row=row+10, column=0, columnspan=4, sticky="w")

        from pump_debug_characterization import CharacterizationPanel
        self.characterization = CharacterizationPanel(self, notebook)

    def table(self, parent, row, keys, titles, height):
        frame = ttk.Frame(parent)
        frame.grid(row=row, column=0, columnspan=4, sticky="nsew", pady=4)
        table = ttk.Treeview(frame, columns=keys, show="headings", height=height)
        scroll = ttk.Scrollbar(frame, orient="vertical", command=table.yview)
        scroll.pack(side="right", fill="y")
        table.pack(fill="both", expand=True)
        table.configure(yscrollcommand=scroll.set)
        for key, title in zip(keys, titles):
            table.heading(key, text=title)
            table.column(key, width=140, minwidth=60)
        return table

    def config(self):
        params = self.app._read_params()
        if not params:
            raise ValueError("Ongeldige oorspronkelijke motorinstellingen")
        node, motor, _, speed, accel, decel = params
        keys = ("pump_type", "pump_id", "stroke_setting", "steps_rev", "fluid", "density", "temperature_c",
                "density_source", "scale_id", "scale_resolution_g", "tubing")
        return PumpConfig(node=node, motor=motor, speed=speed, accel=accel, decel=decel,
                          priming_confirmed=self.confirmed.get(), **{key:self.fields[key].get() for key in keys})

    def make_measurement(self):
        policy = MeasurementPolicy(window_s=self.fields["window"].get(), range_g=self.fields["range"].get(),
                                   timeout_s=self.fields["timeout"].get(), settle_min_s=self.fields["settle"].get())
        adapter = LegacyPumpAdapter(self.app, self.config(), self.confirmed.get())
        # read_scale owns the original scale_lock and updates original live display.
        reader = SimpleNamespace(get_reading=self.app.read_scale)
        return Measurements(adapter, reader, threading.Lock(),
                            lambda kind, detail: self.app._event("TEST", None, b"", f"{kind}: {detail}"),
                            self.directory, policy=policy)

    def options(self):
        def numbers(key):
            return [float(v.strip().replace(",", ".")) for v in self.fields[key].get().split(";")]
        return (numbers("counts"), integer(self.fields["repeats"].get(), "Herhalingen", 100),
                numbers("rpms") if self.fields["rpms"].get().strip() else None,
                numbers("phases") if self.fields["phases"].get().strip() else None,
                self.fields["reference"].get().strip() or None, self.fields["blank"].get().strip() or None)

    def preview(self):
        try:
            measurement = self.make_measurement()
            plan = measurement.plan(*self.options())
            self.show_plan(plan)
        except (ValueError, TypeError) as exc:
            messagebox.showerror("Testpreview", str(exc))

    def show_plan(self, plan):
        self.plan_table.delete(*self.plan_table.get_children())
        for row in plan["rows"]:
            self.plan_table.insert("", "end", values=(row["kind"], row["requested_rev"], f"{row['rpm']:.4g}",
                                                     "vrij" if row["phase"] is None else row["phase"], row["repeat"] or "—", row["phase_shift_steps"]))
        self.progress.set(f"{plan['dose_count']} doseringen · {plan['phase_shift_count']} faseverplaatsingen · {plan['total_rev']:.3f} omw totaal · ~{plan['estimated_s']/60:.1f} min. Fasevloeistof blijft in opvang, buiten dosismassa.")

    def set_locked(self, locked):
        # Existing methods remain unchanged. Lock other controls only during new tests.
        if not locked:
            for widget, state in self.locked_widgets:
                try:
                    if widget.winfo_exists():
                        widget.configure(state=state)
                except tk.TclError:
                    pass
            self.locked_widgets = []
            return
        def visit(parent):
            for widget in parent.winfo_children():
                if isinstance(widget, (ttk.Button, tk.Button, ttk.Entry, ttk.Combobox, ttk.Checkbutton, tk.Checkbutton)):
                    text = str(widget.cget("text")) if "text" in widget.keys() else ""
                    if text.upper().startswith(("STOP", "ABORT", "ANNULEER")):
                        continue
                    self.locked_widgets.append((widget, str(widget.cget("state"))))
                    widget.configure(state="disabled")
                visit(widget)
        visit(self.app.root)

    def start(self, kind):
        if self.busy or self.app.busy:
            messagebox.showwarning("Test", "Er loopt al een activiteit")
            return
        try:
            if not self.app.bus or not self.app.scale:
                raise ValueError("Eerst verbinden via oorspronkelijke CAN- en weegschaalbediening")
            if kind != "blank" and not self.confirmed.get():
                raise ValueError("Bevestig: controller bedrijfsklaar, pomp geprimed en opvang gereed")
            measurement = self.make_measurement()
            if kind == "protocol":
                options = self.options()
                self.show_plan(measurement.plan(*options))
                action = lambda: measurement.protocol(*options)
            elif kind == "blank":
                duration = positive(self.fields["blank"].get() or 5, "Blancoduur")
                action = lambda: measurement.blank(duration)
            elif kind == "calibration":
                rev = integer(self.fields["cal_rev"].get(), "Gehele omwentelingen", 10000)
                repeats = integer(self.fields["cal_repeats"].get(), "Metingen", 100)
                action = lambda: measurement.calibrate(rev, repeats)
            else:
                raise ValueError("Onbekende test")
        except (ValueError, TypeError) as exc:
            messagebox.showerror("Test", str(exc))
            return
        self.clear_results()
        self.measurement, self.busy = measurement, True
        self.profile = None
        self.app.fo_cancel.clear()
        self.app.busy = "forward-only gravimetrische test"
        self.set_locked(True)
        measurement.progress_sink = lambda text: self.app._ui(lambda t=text: self.progress.set(t))
        measurement.result_sink = lambda row: self.app._ui(lambda r=dict(row): self.display_result(r))
        def work():
            error, result = "", None
            try:
                result = action()
                if kind == "calibration":
                    save_profile(result, self.directory)
            except Exception as exc:
                error = str(exc)
                self.app._event("ERR", None, b"", error)
                # Original stop implementation; do not substitute another command.
                self.app.cancel_forward_only()
            finally:
                self.app._ui(lambda e=error, r=result: self.finish(e, r, kind))
        self.worker = threading.Thread(target=work, daemon=True)
        try:
            self.worker.start()
        except Exception as exc:
            self.finish(str(exc), None, kind)

    def finish(self, error, result, kind):
        self.busy = False
        self.app.busy = None
        self.set_locked(False)
        self.confirmed.set(False)  # Operator confirms again before the next new test.
        if not error and kind == "calibration":
            self.profile = result
            self.progress.set(f"Gemeten {result['g_per_rev']:.6f} g/omw; profiel opgeslagen")
        else:
            self.progress.set(f"Test gestopt: {error}" if error else "Test afgerond; CSV/JSON en journal opgeslagen")
        self.progress_bar["value"] = 0 if error else 100
        if self.measurement:
            self.show_summary(self.measurement.analysis())
            self.update_samples()
        if error:
            messagebox.showerror("Test gestopt", error)

    def apply_calibration(self):
        try:
            if not self.profile:
                raise ValueError("Eerst gravimetrisch kalibreren in dit testtabblad")
            validate_profile(self.profile, self.config())
            self.app.fo_gpr.set(f"{self.profile['g_per_rev']:.8f}")
            self.progress.set("Gemeten g/omw ingevuld in oorspronkelijke Forward-only-tab")
        except ValueError as exc:
            messagebox.showerror("Kalibratie", str(exc))

    def display_result(self, row):
        self.display_records.append(row)
        def fmt(key):
            value = row.get(key)
            return f"{value:.5f}" if isinstance(value, (int, float)) else "—"
        self.results_table.insert("", "end", values=(row["kind"], fmt("rev"), fmt("start_phase_rev"), fmt("mass_g"), fmt("volume_ml"), row["status"], fmt("elapsed_s")))
        if row.get("total"):
            self.progress_bar["value"] = 100*row["sequence"]/row["total"]
        if self.measurement:
            self.update_samples()

    def update_samples(self):
        samples = self.measurement.scale_samples
        stride = max(1, (len(samples)+1199)//1200)
        self.display_samples = samples[::stride]
        self.draw_graph()

    def clear_results(self):
        self.display_records, self.display_samples, self.display_summary = [], [], []
        self.results_table.delete(*self.results_table.get_children())
        self.show_summary([])
        self.draw_graph()

    def open_results(self):
        if self.busy or self.app.busy:
            return
        path = filedialog.askopenfilename(title="Testresultaten / journal", filetypes=[("Resultaten", "*.json *.jsonl")])
        if not path:
            return
        try:
            payload = load_results(path)
            self.clear_results()
            self.measurement = None
            for row in payload["records"]:
                self.display_result(row)
            self.display_samples = payload["scale_samples"]
            self.show_summary(payload["analysis"])
            self.draw_graph()
        except (ValueError, OSError, KeyError, TypeError) as exc:
            messagebox.showerror("Resultaten", str(exc))

    def show_summary(self, summaries):
        for item in self.summary_table.get_children():
            self.summary_table.delete(item)
        for row in summaries:
            def fmt(key):
                value = row.get(key)
                return f"{value:.5g}" if isinstance(value, (int, float)) else "—"
            self.summary_table.insert("", "end", values=(row["kind"], fmt("rev"), fmt("rpm"), fmt("start_phase_rev"), row["n"], fmt("mean_g"), fmt("sd_g"), fmt("cv_pct")))
        self.summary_text.set(f"{len(summaries)} afzonderlijke groepen; gegroepeerd op testtype, stappen, toerental en startfase bij deeldoses. SD/CV vereisen herhalingen.")
        self.display_summary = summaries


    def draw_graph(self):
        canvas = self.graph
        canvas.delete("all")
        width, height = max(canvas.winfo_width(), 400), max(canvas.winfo_height(), 160)
        mode = self.graph_mode.get()
        points = []
        if mode == "Gewicht / tijd":
            samples = self.display_samples
            stride = max(1, (len(samples)+1199)//1200)
            for row in samples[::stride]:
                if isinstance(row.get("g"), (int, float)) and row.get("elapsed_session_s") is not None:
                    points.append((row["elapsed_session_s"], row["g"], row.get("stage", "unknown"),
                                   f"{row.get('measurement_id', '—')} · {row.get('stage', '—')} · {row['g']:.5f} g"))
            xlabel, ylabel = "Tijd sinds sessiestart (s)", "Schaalmassa (g)"
        elif mode == "CV / testgroep":
            for index, row in enumerate(getattr(self, "display_summary", []), 1):
                if row.get("cv_pct") is not None:
                    points.append((index, row["cv_pct"], f"{row['rpm']:g} rpm",
                                   f"{row['kind']} · {row['rev']:g} omw · fase {row['start_phase_rev']} · CV {row['cv_pct']:.3f}%"))
            xlabel, ylabel = "Afzonderlijke testgroep", "CV (%)"
        else:
            for row in self.display_records:
                if row.get("status") != "completed" or row.get("mass_g") is None or row["kind"] not in ("PROTOCOL", "REFERENCE", "CALIBRATION", "DOSE"):
                    continue
                if mode == "Massa / startfase" and (not row.get("steps") or row["steps"] % row["context"]["steps_rev"] == 0):
                    continue
                x = row["start_phase_rev"] if mode == "Massa / startfase" else row["rev"]
                group = f"{row['rpm']:g} rpm / {row['rev']:g} omw" if mode == "Massa / startfase" else f"{row['rpm']:g} rpm"
                points.append((x, row["mass_g"], group,
                               f"{row['kind']} · {row['rev']:g} omw · {row['rpm']:g} rpm · fase {row['start_phase_rev']:.4f} · {row['mass_g']:.5f} g"))
            xlabel, ylabel = ("Startfase relatief aan softwareteller (omw)" if mode == "Massa / startfase" else "Gecommandeerde pompomwentelingen"), "Gemeten massa (g)"
        canvas.create_text(12, 10, anchor="nw", text=ylabel, fill="#334155")
        if not points:
            canvas.create_text(width/2, height/2, text="Nog geen geschikte metingen voor deze grafiek", fill="#64748b")
            return
        left, right, top, bottom = 65, width-20, 35, height-42
        xmin, xmax = min(p[0] for p in points), max(p[0] for p in points)
        ymin, ymax = min(p[1] for p in points), max(p[1] for p in points)
        xpad = (xmax-xmin)*0.06 or max(abs(xmin)*0.05, 0.05)
        ypad = (ymax-ymin)*0.08 or max(abs(ymin)*0.05, 0.001)
        xmin, xmax, ymin, ymax = xmin-xpad, xmax+xpad, ymin-ypad, ymax+ypad
        canvas.create_line(left, top, left, bottom, right, bottom, fill="#94a3b8")
        for index in range(5):
            t = index/4
            y = bottom-t*(bottom-top)
            canvas.create_line(left, y, right, y, fill="#e2e8f0")
            canvas.create_text(left-7, y, anchor="e", text=f"{ymin+t*(ymax-ymin):.4g}", fill="#64748b")
            canvas.create_text(left+t*(right-left), bottom+12, text=f"{xmin+t*(xmax-xmin):.4g}", fill="#64748b")
        canvas.create_text((left+right)/2, height-8, text=xlabel, fill="#334155")
        colors = ("#2563eb", "#059669", "#d97706", "#9333ea", "#dc2626", "#0891b2")
        groups = list(dict.fromkeys(p[2] for p in points))
        for x, y, group, detail in points:
            px, py = left+(x-xmin)/(xmax-xmin)*(right-left), bottom-(y-ymin)/(ymax-ymin)*(bottom-top)
            dot = canvas.create_oval(px-3, py-3, px+3, py+3, fill=colors[groups.index(group)%len(colors)], outline="")
            canvas.tag_bind(dot, "<Enter>", lambda event, text=detail: self.graph_hint.set(text))
        self.graph_hint.set("Punten = afzonderlijke metingen; details bij aanwijzen. Tijdgrafiek beperkt tot ~1200 punten; export bevat alle samples. Geen lineaire fit.")

