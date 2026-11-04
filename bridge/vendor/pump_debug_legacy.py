"""Pump Debug - standalone VWA pump motion + CAN + scale + calibration + dose-test tool.

Talks to the IXXAT adapter directly through python-can, independent of the
backend, so every TX/RX/error frame is visible. The scale is driven through the
app's own ScaleCommunicator and calibration runs the app's own
VWACalibrationController, so results match the backend routine. Stability is
taken from the MT-SICS status character ('S S' stable / 'S D' dynamic), because
the shared parser reports every 'S x' line as stable.

Tabs: pump/CAN terminal, calibration (+ dose a requested weight from the active
calibration), a configurable dose test, and forward-only dosing for ceramic
piston pumps (calibrate g/rev with scale → learn history → production open-loop
with optional scale check). Everything is written below logs/pump_debug, never
to the app database (which is only read, read-only, to load a calibration).

Run:  .venv\\Scripts\\python.exe tools\\pump_debug.py
The backend must NOT be connected to CAN or the scale at the same time (one
owner of the IXXAT controller and of the COM port).
"""

import asyncio
import copy
import csv
import json
import logging
import math
import os
import queue
import re
import sqlite3
import statistics
import sys
import threading
import time
from datetime import datetime
from pathlib import Path
import tkinter as tk
from tkinter import filedialog, messagebox, ttk

import can
import serial.tools.list_ports

REPO_ROOT = Path(__file__).resolve().parents[1]
# Reason: reuse the backend's scale + calibration code instead of a second copy that could drift.
sys.path[:0] = [str(REPO_ROOT / "src"), str(REPO_ROOT / "backend"), str(Path(__file__).resolve().parent)]
from utils.scale_communication import ScaleCommunicator  # noqa: E402
from vwa_calibration import VWACalibrationController  # noqa: E402
import pump_debug_dosing as dosing  # noqa: E402
import pump_debug_forward as fo  # noqa: E402

STEPS_PER_REV = 51200
FRAME_GAP_S = 0.050  # VWA protocol: 50 ms between frames to same node
LOG_DIR = REPO_ROOT / "logs" / "pump_debug"
CAL_DIR = LOG_DIR / "calibrations"
DOSE_DIR = LOG_DIR / "dose_tests"
PLAN_DIR = LOG_DIR / "dose_plans"
FO_DIR = LOG_DIR / "forward_only"
APP_DB = REPO_ROOT / "data" / "octopus_data.db"
SETTINGS_FILE = REPO_ROOT / "config" / "settings.json"
MTSICS_WEIGHT = re.compile(r"^\s*S\s+([SD])\s+[-+]?\s*\d")  # 'S S   12.345 g' / 'S D   12.345 g'
SCALE_POLL_S = 0.5
MOVE_DONE_CODES = (0x4A, 0x4B)  # board echoes MOVE_TO / MOVE_TO_ZERO with rec=1 when motion ends
POSITION_CODES = (0x01, 0x4A, 0x4B, 0x4C)  # include ZERO_DONE so counter reset is visible

COMMANDS = {
    "START": 0x01, "STOP": 0x08, "ABORT": 0x05, "HOME": 0x16, "INIT": 0x46,
    "READ_STATUS": 0x47, "MOVE_TO": 0x4A, "MOVE_TO_ZERO": 0x4B, "ZERO": 0x4C,
}
RESPONSES = {
    1: "UPDATE", 2: "STARTED/COMPLETED", 9: "STOPPED", 10: "ABORTED", 11: "PAUSED",
    66: "SET_POSITION", 70: "INIT_DONE", 71: "READ_STATUS", 72: "UPDATE_STATUS",
    73: "DISPENSE_DONE", 74: "MOVE_TO_DONE", 75: "MOVE_TO_ZERO_DONE",
    76: "ZERO_DONE", 77: "HOME_DONE", 0xFE: "HEARTBEAT", 0xFF: "ERROR",
}
CMD_NAMES = {v: k for k, v in COMMANDS.items()}


def decode(can_id: int, data: bytes, direction: str) -> str:
    """Human-readable VWA interpretation of one frame."""
    if len(data) < 4:
        return ""
    if data[0] == 0xFF and data[1] == 0xFF and data[2] == 0x00 and data[3] == 0xAA:
        counter = int.from_bytes(data[4:8], "little") if len(data) >= 8 else 0
        return f"HEARTBEAT node {can_id} #{counter}"
    if len(data) < 8:
        return ""
    code, rec, disp, valid = data[0], data[1], data[2], data[3]
    value = int.from_bytes(data[4:8], "little", signed=True)
    if direction == "TX":
        name = CMD_NAMES.get(code, f"0x{code:02X}") if code else "param"
    else:
        name = RESPONSES.get(code, f"0x{code:02X}") if code else "status"
    flag = "" if valid in (0xAA, 0xBB) else f" (validation 0x{valid:02X}?)"
    return f"{name} rec={rec} motor={disp} val={value}{flag}"


def parse_position_feedback(data: bytes):
    """Return (motor, position, is_move_done) for a board position frame, else None."""
    if len(data) < 8 or data[1] != 1 or data[3] != 0xAA or data[0] not in POSITION_CODES:
        return None
    position = int.from_bytes(data[4:8], "little", signed=True)
    return data[2], position, data[0] in MOVE_DONE_CODES


def move_timeout(distance: int, speed: int, accel: int, decel: int, margin: float = 3.0) -> float:
    """Worst-case seconds for a trapezoidal move, plus margin for CAN/firmware latency."""
    speed = max(speed, 1)
    return abs(distance) / speed + speed / max(accel, 1) + speed / max(decel, 1) + margin


def safe_name(text: str) -> str:
    """Filename-safe lowercase component (fluid names come from free text)."""
    return re.sub(r"[^a-z0-9_-]+", "_", text.strip().lower()).strip("_") or "fluid"


def calibration_csv_rows(result):
    """Flatten per-run points plus the averaged table into CSV rows (mg values)."""
    rows = []
    sources = [(str(run.run_number), run.points) for run in result.runs]
    sources.append(("avg", result.calibration_table))
    for label, points in sources:
        for p in points:
            rows.append([label, p.index, round(p.angle, 3), p.steps, round(p.position, 6),
                         round(p.dispensedF, 3), round(p.dispensedR, 3), round(p.dispensed, 3)])
    return rows


def mtsics_stable(raw: str):
    """True for 'S S', False for 'S D', None when the reply has no MT-SICS weight status."""
    for line in (raw or "").splitlines():
        match = MTSICS_WEIGHT.match(line)
        if match:
            return match.group(1) == "S"
    return None


class Cancelled(Exception):
    """Raised inside a worker when the user cancels (button or Esc)."""


def load_scale_defaults():
    """Scale port/baud from config/settings.json, falling back to the app defaults."""
    try:
        with open(SETTINGS_FILE, encoding="utf-8") as f:
            scale = json.load(f).get("scale_settings", {})
    except (OSError, ValueError):
        scale = {}
    return str(scale.get("port", "COM20")), str(scale.get("baudrate", 9600))


class GuiLogHandler(logging.Handler):
    """Route library log records (python-can errors, calibration progress) into the GUI."""

    def __init__(self, sink, direction="ERR", level=logging.WARNING):
        super().__init__(level)
        self.sink = sink
        self.direction = direction

    def emit(self, record):
        direction = "ERR" if record.levelno >= logging.WARNING else self.direction
        self.sink(direction, None, b"", record.getMessage())


class PumpDebugApp:
    def __init__(self, root: tk.Tk):
        self.root = root
        self.bus = None
        self.rx_thread = None
        self.running = False
        self.events = queue.Queue()
        self.send_lock = threading.Lock()
        self.stats = {"TX": 0, "RX": 0, "ERR": 0, "HB": 0}
        self.last_heartbeat = {}
        self.commanded = {m: 0 for m in range(1, 9)}
        self.reported = {}  # (node, motor) -> last position the board reported
        self.rows = []
        self.csv_file = None
        self.csv_writer = None
        self.ui_calls = queue.Queue()  # callables from worker threads, run on the Tk thread

        # Scale: one lock serialises poller, buttons and calibration on the serial port.
        self.scale = None
        self.scale_lock = threading.Lock()
        self.scale_running = False
        self.scale_thread = None
        self.scale_reading = None
        self.scale_reading_at = 0.0
        self.scale_status_known = False  # True when the last reply carried an MT-SICS S/D status
        self.scale_live_on = True
        self.scale_log_on = False

        # One motion+scale activity at a time ("kalibratie", "dosering", "doseertest"); the poller pauses meanwhile.
        self.busy = None
        # Calibration: move_wait is (node, motor) while a move waits for its done frame.
        self.calibration = None
        self.cal_cancel = threading.Event()
        self.cal_ramp = (51200, 51200)
        self.cal_wait_done = True
        self.move_wait = None
        self.move_done = threading.Event()
        self.move_done_pos = 0

        # Active calibration used for dosing, and dose-test state.
        self.active_cal = None
        self.active_cal_src = ""
        self.dose_cancel = threading.Event()
        self.dose_params = None
        self.plan = [(10.0, 10), (20.0, 10)]
        self.dose_records = []
        self.last_report = None
        self.fo_cancel = threading.Event()
        self.fo_records = []
        self.fo_primed = False  # True after a successful FO prime this session
        self.fo_phase = 0  # 0 = software zero at pump phase 0; else offset after last dose
        self.fo_base_yield = 1.0  # EMA dispensed/aim of base stroke
        self.fo_cal = None
        self.fo_cal_path = None
        self.fo_history = []  # dose records for review

        root.title("Pump Debug - VWA CAN + weegschaal + kalibratie + doseertest")
        root.geometry("1220x980")
        self._build_ui()
        logging.getLogger("can").addHandler(GuiLogHandler(self._event))
        cal_logger = logging.getLogger("vwa_calibration")
        cal_logger.setLevel(logging.INFO)
        cal_logger.addHandler(GuiLogHandler(self._event, direction="CAL", level=logging.INFO))
        root.bind("<Escape>", lambda e: self.abort_all())
        root.protocol("WM_DELETE_WINDOW", self.on_close)
        root.after(50, self._drain_events)
        root.after(500, self._refresh_stats)

    # ---------- UI ----------

    def _build_ui(self):
        top = ttk.LabelFrame(self.root, text="CAN verbinding")
        top.pack(fill="x", padx=8, pady=4)
        self.iface = tk.StringVar(value="ixxat")
        self.channel = tk.StringVar(value="0")
        self.bitrate = tk.StringVar(value="250000")
        ttk.Label(top, text="Interface").grid(row=0, column=0, padx=4)
        ttk.Combobox(top, textvariable=self.iface, values=["ixxat", "virtual"], width=8).grid(row=0, column=1)
        ttk.Label(top, text="Kanaal").grid(row=0, column=2, padx=4)
        ttk.Combobox(top, textvariable=self.channel, values=["0", "1"], width=4).grid(row=0, column=3)
        ttk.Label(top, text="Bitrate").grid(row=0, column=4, padx=4)
        ttk.Combobox(top, textvariable=self.bitrate,
                     values=["125000", "250000", "500000", "1000000"], width=9).grid(row=0, column=5)
        self.connect_btn = ttk.Button(top, text="Verbinden", command=self.toggle_connect)
        self.connect_btn.grid(row=0, column=6, padx=8)
        self.status_lbl = ttk.Label(top, text="Niet verbonden", foreground="gray")
        self.status_lbl.grid(row=0, column=7, padx=8)
        self.stats_lbl = ttk.Label(top, text="", font=("Consolas", 10))
        self.stats_lbl.grid(row=0, column=8, padx=8, sticky="w")
        top.columnconfigure(8, weight=1)

        self._build_scale_ui(self.root)

        notebook = ttk.Notebook(self.root)
        notebook.pack(fill="x", padx=8, pady=4)
        mid = ttk.Frame(notebook)
        cal_tab = ttk.Frame(notebook)
        dose_tab = ttk.Frame(notebook)
        fo_tab = ttk.Frame(notebook)
        notebook.add(mid, text="Pomp & CAN-terminal")
        notebook.add(cal_tab, text="Kalibratie & doseren")
        notebook.add(dose_tab, text="Doseertest")
        notebook.add(fo_tab, text="Forward-only")

        pump = ttk.LabelFrame(mid, text="Pomp")
        pump.pack(side="left", fill="y", padx=(0, 4), pady=4)
        self.node = tk.StringVar(value="610")
        self.motor = tk.StringVar(value="1")
        self.position = tk.StringVar(value="51200")
        self.speed = tk.StringVar(value="51200")
        self.accel = tk.StringVar(value="51200")
        self.decel = tk.StringVar(value="51200")
        self.jog = tk.StringVar(value="0.25")
        fields = [("Node ID", self.node), ("Motor (1-8)", self.motor), ("Positie (stappen)", self.position),
                  ("Snelheid (stap/s)", self.speed), ("Acceleratie", self.accel), ("Deceleratie", self.decel),
                  ("Jog (omw)", self.jog)]
        for r, (label, var) in enumerate(fields):
            ttk.Label(pump, text=label).grid(row=r, column=0, sticky="w", padx=4, pady=1)
            ttk.Entry(pump, textvariable=var, width=12).grid(row=r, column=1, padx=4, pady=1)
        self.pos_hint = ttk.Label(pump, text="", foreground="gray")
        self.pos_hint.grid(row=2, column=2, sticky="w")
        self.position.trace_add("write", lambda *a: self._update_hint())
        self._update_hint()

        btns = ttk.Frame(pump)
        btns.grid(row=len(fields), column=0, columnspan=3, pady=6)
        actions = [
            ("INIT", lambda: self.command("INIT")), ("START", lambda: self.command("START")),
            ("INIT + START", self.init_start), ("MOVE_TO", self.move_to),
            ("MOVE_TO_ZERO", lambda: self.command("MOVE_TO_ZERO", 0)), ("HOME", lambda: self.command("HOME")),
            ("ZERO (pos=0)", lambda: self.command("ZERO")), ("READ_STATUS", lambda: self.command("READ_STATUS")),
            ("◀ Jog -", lambda: self.jog_move(-1)), ("Jog + ▶", lambda: self.jog_move(1)),
            ("STOP", lambda: self.command("STOP")), ("ABORT (Esc = alle)", lambda: self.command("ABORT")),
        ]
        for i, (text, fn) in enumerate(actions):
            ttk.Button(btns, text=text, command=fn, width=16).grid(row=i // 2, column=i % 2, padx=2, pady=2)
        self.cmd_pos_lbl = ttk.Label(pump, text="", foreground="#555")
        self.cmd_pos_lbl.grid(row=len(fields) + 1, column=0, columnspan=3, sticky="w", padx=4)

        raw = ttk.LabelFrame(mid, text="Terminal (raw frame / VWA sequence)")
        raw.pack(side="left", fill="both", expand=True, padx=4, pady=4)
        self.raw_id = tk.StringVar(value="610")
        self.raw_data = tk.StringVar(value="00 08 01 BB 00 00 00 00")
        ttk.Label(raw, text="ID (dec, of 0x..)").grid(row=0, column=0, sticky="w", padx=4)
        ttk.Entry(raw, textvariable=self.raw_id, width=10).grid(row=0, column=1, sticky="w")
        ttk.Label(raw, text="Data (hex)").grid(row=1, column=0, sticky="w", padx=4)
        raw_entry = ttk.Entry(raw, textvariable=self.raw_data, width=40, font=("Consolas", 10))
        raw_entry.grid(row=1, column=1, sticky="we")
        raw_entry.bind("<Return>", lambda e: self.send_raw())
        ttk.Button(raw, text="Verstuur frame (Enter)", command=self.send_raw).grid(row=1, column=2, padx=4)
        ttk.Label(raw, text="Custom VWA cmd (hex)").grid(row=2, column=0, sticky="w", padx=4)
        self.custom_cmd = tk.StringVar(value="4A")
        ttk.Entry(raw, textvariable=self.custom_cmd, width=6).grid(row=2, column=1, sticky="w")
        ttk.Button(raw, text="Verstuur 8-frame sequence", command=self.send_custom).grid(row=2, column=2, padx=4)
        ttk.Label(raw, text="Sequence gebruikt node/motor/positie/snelheid uit 'Pomp'. "
                            "Esc = ABORT motor 1-8 op node.", foreground="gray").grid(
            row=3, column=0, columnspan=3, sticky="w", padx=4, pady=4)
        raw.columnconfigure(1, weight=1)

        self._build_calibration_ui(cal_tab)
        self._build_active_cal_ui(cal_tab)
        self._build_dose_ui(dose_tab)
        self._build_forward_only_ui(fo_tab)

        logf = ttk.LabelFrame(self.root, text="Busverkeer")
        logf.pack(fill="both", expand=True, padx=8, pady=4)
        bar = ttk.Frame(logf)
        bar.pack(fill="x")
        self.show_hb = tk.BooleanVar(value=True)
        self.show_tx = tk.BooleanVar(value=True)
        self.autoscroll = tk.BooleanVar(value=True)
        self.paused = tk.BooleanVar(value=False)
        ttk.Checkbutton(bar, text="Heartbeats", variable=self.show_hb).pack(side="left", padx=4)
        ttk.Checkbutton(bar, text="TX tonen", variable=self.show_tx).pack(side="left", padx=4)
        ttk.Checkbutton(bar, text="Autoscroll", variable=self.autoscroll).pack(side="left", padx=4)
        ttk.Checkbutton(bar, text="Pauze weergave", variable=self.paused).pack(side="left", padx=4)
        ttk.Button(bar, text="Wissen", command=self.clear_log).pack(side="right", padx=4)
        ttk.Button(bar, text="Opslaan als CSV…", command=self.save_csv).pack(side="right", padx=4)
        ttk.Button(bar, text="Notitie", command=self.add_note).pack(side="right", padx=4)
        self.hb_lbl = ttk.Label(bar, text="", foreground="#555")
        self.hb_lbl.pack(side="left", padx=12)

        cols = ("time", "dir", "id", "data", "info")
        self.tree = ttk.Treeview(logf, columns=cols, show="headings", height=10)
        for col, width, text in [("time", 105, "Tijd"), ("dir", 45, "Dir"), ("id", 90, "ID"),
                                 ("data", 230, "Data"), ("info", 600, "Decode")]:
            self.tree.heading(col, text=text)
            self.tree.column(col, width=width, anchor="w", stretch=(col == "info"))
        sb = ttk.Scrollbar(logf, orient="vertical", command=self.tree.yview)
        self.tree.configure(yscrollcommand=sb.set)
        self.tree.pack(side="left", fill="both", expand=True)
        sb.pack(side="right", fill="y")
        for tag, color in [("TX", "#1a5fb4"), ("RX", "#26a269"), ("ERR", "#c01c28"),
                           ("INFO", "#555555"), ("HB", "#9a9996"), ("SCALE", "#813d9c"),
                           ("CAL", "#b5835a"), ("DOSE", "#1c71d8"), ("FO", "#0a7d6c")]:
            self.tree.tag_configure(tag, foreground=color)

        from pump_debug_tests import LegacyTestPanel
        self.gravimetric_tests = LegacyTestPanel(self, notebook)

    def _build_scale_ui(self, parent):
        box = ttk.LabelFrame(parent, text="Weegschaal")
        box.pack(fill="x", padx=8, pady=4)
        port, baud = load_scale_defaults()
        self.scale_mode = tk.StringVar(value="Serieel")
        self.scale_port = tk.StringVar(value=port)
        self.scale_baud = tk.StringVar(value=baud)
        self.scale_host = tk.StringVar(value="")
        self.scale_tcp_port = tk.StringVar(value="8001")

        ttk.Label(box, text="Type").grid(row=0, column=0, sticky="w", padx=4)
        ttk.Combobox(box, textvariable=self.scale_mode, values=["Serieel", "TCP"], width=8,
                     state="readonly").grid(row=0, column=1, sticky="w")
        ttk.Label(box, text="COM").grid(row=0, column=2, sticky="w", padx=4)
        self.scale_port_box = ttk.Combobox(box, textvariable=self.scale_port, width=8,
                                           postcommand=self._refresh_com_ports)
        self.scale_port_box.grid(row=0, column=3, sticky="w")
        ttk.Label(box, text="Baud").grid(row=0, column=4, sticky="w", padx=4)
        ttk.Combobox(box, textvariable=self.scale_baud, values=["2400", "4800", "9600", "19200"],
                     width=6).grid(row=0, column=5, sticky="w", padx=(0, 4))
        ttk.Label(box, text="TCP host").grid(row=0, column=6, sticky="w", padx=4)
        ttk.Entry(box, textvariable=self.scale_host, width=14).grid(row=0, column=7, sticky="w")
        ttk.Label(box, text="Poort").grid(row=0, column=8, sticky="e", padx=4)
        ttk.Entry(box, textvariable=self.scale_tcp_port, width=6).grid(row=0, column=9, sticky="w")
        self.scale_btn = ttk.Button(box, text="Verbinden", command=self.toggle_scale)
        self.scale_btn.grid(row=0, column=10, sticky="w", padx=8, pady=2)
        self.scale_status_lbl = ttk.Label(box, text="Niet verbonden", foreground="gray")
        self.scale_status_lbl.grid(row=0, column=11, sticky="w")

        self.weight_lbl = tk.Label(box, text="---.---- g", font=("Consolas", 18, "bold"), fg="gray")
        self.weight_lbl.grid(row=1, column=0, columnspan=6, sticky="w", padx=4)

        btns = ttk.Frame(box)
        btns.grid(row=1, column=6, columnspan=6, sticky="w", padx=2, pady=2)
        ttk.Button(btns, text="Tarra (ZI)", command=lambda: self.scale_action("tare")).pack(side="left", padx=2)
        ttk.Button(btns, text="Nul (Z)", command=lambda: self.scale_action("zero")).pack(side="left", padx=2)
        ttk.Button(btns, text="Lees (SI)", command=lambda: self.scale_action("read")).pack(side="left", padx=2)
        self.scale_live = tk.BooleanVar(value=True)
        self.scale_log = tk.BooleanVar(value=False)
        ttk.Checkbutton(btns, text="Live", variable=self.scale_live,
                        command=lambda: setattr(self, "scale_live_on", self.scale_live.get())).pack(side="left", padx=4)
        ttk.Checkbutton(btns, text="Log in busverkeer", variable=self.scale_log,
                        command=lambda: setattr(self, "scale_log_on", self.scale_log.get())).pack(side="left")

    def _build_calibration_ui(self, parent):
        box = ttk.LabelFrame(parent, text="Kalibratie (VWA-routine van de app)")
        box.pack(side="left", fill="y", padx=4, pady=4)
        self.cal_fluid = tk.StringVar(value="water")
        self.cal_samples = tk.StringVar(value="30")
        self.cal_angle = tk.StringVar(value="210")
        self.cal_runs = tk.StringVar(value="2")
        self.cal_reverse = tk.BooleanVar(value=True)
        self.cal_init = tk.BooleanVar(value=True)
        self.cal_wait = tk.BooleanVar(value=True)

        fields = [("Vloeistof", self.cal_fluid, 14), ("Samples", self.cal_samples, 6),
                  ("Max hoek (°)", self.cal_angle, 6), ("Runs", self.cal_runs, 6)]
        for i, (label, var, width) in enumerate(fields):
            ttk.Label(box, text=label).grid(row=i // 2, column=(i % 2) * 2, sticky="w", padx=4, pady=1)
            ttk.Entry(box, textvariable=var, width=width).grid(row=i // 2, column=(i % 2) * 2 + 1, sticky="w")
        checks = ttk.Frame(box)
        checks.grid(row=2, column=0, columnspan=4, sticky="w", padx=2)
        ttk.Checkbutton(checks, text="Terug naar nul meten (reverse)", variable=self.cal_reverse).pack(side="left", padx=2)
        ttk.Checkbutton(checks, text="INIT + START vooraf", variable=self.cal_init).pack(side="left", padx=2)
        ttk.Checkbutton(checks, text="Wacht op MOVE_TO_DONE", variable=self.cal_wait).pack(side="left", padx=2)

        btns = ttk.Frame(box)
        btns.grid(row=3, column=0, columnspan=4, sticky="w", padx=2, pady=2)
        self.cal_start_btn = ttk.Button(btns, text="Start kalibratie", command=self.start_calibration)
        self.cal_start_btn.pack(side="left", padx=2)
        self.cal_cancel_btn = ttk.Button(btns, text="Annuleren", command=self.cancel_calibration, state="disabled")
        self.cal_cancel_btn.pack(side="left", padx=2)
        ttk.Button(btns, text="Map openen", command=self.open_cal_dir).pack(side="left", padx=2)

        self.cal_progress = ttk.Progressbar(box, maximum=100)
        self.cal_progress.grid(row=4, column=0, columnspan=4, sticky="we", padx=4, pady=2)
        self.cal_status_lbl = ttk.Label(box, text="Node/motor/snelheid/accel/decel komen uit 'Pomp'.",
                                        foreground="gray")
        self.cal_status_lbl.grid(row=5, column=0, columnspan=4, sticky="w", padx=4)
        self.cal_result_lbl = ttk.Label(box, text="", font=("Consolas", 10))
        self.cal_result_lbl.grid(row=6, column=0, columnspan=4, sticky="w", padx=4)
        ttk.Label(box, text="Stabiel = weegschaal meldt 'S S' (niet 'S D') en blijft 1 s stabiel; "
                            "na 10 s gaat de app-routine toch door (rode logregel).",
                  foreground="gray", wraplength=520).grid(row=7, column=0, columnspan=4, sticky="w", padx=4, pady=2)
        box.columnconfigure(3, weight=1)

    def _build_active_cal_ui(self, parent):
        box = ttk.LabelFrame(parent, text="Actieve kalibratie & gewicht doseren")
        box.pack(side="left", fill="both", expand=True, padx=4, pady=4)
        btns = ttk.Frame(box)
        btns.pack(fill="x", padx=2, pady=2)
        ttk.Button(btns, text="Laad uit app-database", command=self.load_cal_from_db).pack(side="left", padx=2)
        ttk.Button(btns, text="Laad JSON-bestand…", command=self.load_cal_from_file).pack(side="left", padx=2)
        ttk.Label(box, text="App-database: pomp = node/motor uit 'Pomp', vloeistof uit het kalibratieveld. "
                            "Een nieuwe kalibratie wordt automatisch actief.",
                  foreground="gray", wraplength=520).pack(fill="x", padx=4)
        self.active_cal_lbl = ttk.Label(box, text="Geen kalibratie actief", font=("Consolas", 10),
                                        justify="left")
        self.active_cal_lbl.pack(fill="x", padx=4, pady=4)

        dose = ttk.Frame(box)
        dose.pack(fill="x", padx=2, pady=4)
        self.single_target = tk.StringVar(value="10")
        ttk.Label(dose, text="Gewicht (g)").pack(side="left", padx=2)
        entry = ttk.Entry(dose, textvariable=self.single_target, width=8)
        entry.pack(side="left", padx=2)
        entry.bind("<Return>", lambda e: self.dose_single())
        ttk.Button(dose, text="Doseer gewicht", command=self.dose_single).pack(side="left", padx=4)
        ttk.Label(dose, text="(instellingen van tab Doseertest)", foreground="gray").pack(side="left")
        self.single_result_lbl = ttk.Label(box, text="", font=("Consolas", 10), justify="left")
        self.single_result_lbl.pack(fill="x", padx=4)

    def _build_dose_ui(self, parent):
        plan_box = ttk.LabelFrame(parent, text="Testplan")
        plan_box.pack(side="left", fill="y", padx=4, pady=4)
        self.plan_tree = ttk.Treeview(plan_box, columns=("target", "reps"), show="headings", height=7)
        self.plan_tree.heading("target", text="Doel (g)")
        self.plan_tree.heading("reps", text="Aantal")
        self.plan_tree.column("target", width=90, anchor="e")
        self.plan_tree.column("reps", width=70, anchor="e")
        self.plan_tree.grid(row=0, column=0, columnspan=4, sticky="we", padx=4, pady=2)
        self.plan_tree.bind("<<TreeviewSelect>>", lambda e: self._plan_selected())
        self.plan_target = tk.StringVar(value="10")
        self.plan_reps = tk.StringVar(value="10")
        ttk.Label(plan_box, text="Doel (g)").grid(row=1, column=0, sticky="w", padx=4)
        ttk.Entry(plan_box, textvariable=self.plan_target, width=8).grid(row=1, column=1, sticky="w")
        ttk.Label(plan_box, text="Aantal").grid(row=1, column=2, sticky="w", padx=4)
        ttk.Entry(plan_box, textvariable=self.plan_reps, width=6).grid(row=1, column=3, sticky="w")
        edit = ttk.Frame(plan_box)
        edit.grid(row=2, column=0, columnspan=4, sticky="w", padx=2, pady=2)
        for text, fn in [("Toevoegen", self.plan_add), ("Wijzig", self.plan_update), ("Verwijder", self.plan_remove),
                         ("▲", lambda: self.plan_move(-1)), ("▼", lambda: self.plan_move(1))]:
            ttk.Button(edit, text=text, command=fn, width=9 if len(text) > 1 else 3).pack(side="left", padx=1)
        self.plan_text = tk.StringVar(value=dosing.plan_to_text(self.plan))
        ttk.Label(plan_box, text="Snel invoeren").grid(row=3, column=0, sticky="w", padx=4)
        quick = ttk.Entry(plan_box, textvariable=self.plan_text, width=22)
        quick.grid(row=3, column=1, columnspan=2, sticky="we")
        quick.bind("<Return>", lambda e: self.plan_apply_text())
        ttk.Button(plan_box, text="Toepassen", command=self.plan_apply_text).grid(row=3, column=3, padx=2)
        files = ttk.Frame(plan_box)
        files.grid(row=4, column=0, columnspan=4, sticky="w", padx=2, pady=2)
        ttk.Button(files, text="Plan opslaan…", command=self.plan_save).pack(side="left", padx=1)
        ttk.Button(files, text="Plan laden…", command=self.plan_load).pack(side="left", padx=1)
        self.plan_total_lbl = ttk.Label(plan_box, text="", foreground="#555")
        self.plan_total_lbl.grid(row=5, column=0, columnspan=4, sticky="w", padx=4)

        opts = ttk.LabelFrame(parent, text="Instellingen & uitvoeren")
        opts.pack(side="left", fill="y", padx=4, pady=4)
        self.dose_pause = tk.StringVar(value="2")
        self.dose_stable_timeout = tk.StringVar(value="10")
        self.dose_stable_hold = tk.StringVar(value="1.0")
        fields = [("Pauze tussen doses (s)", self.dose_pause), ("Max wachten stabiel (s)", self.dose_stable_timeout),
                  ("Stabiel aanhouden (s)", self.dose_stable_hold)]
        for r, (label, var) in enumerate(fields):
            ttk.Label(opts, text=label).grid(row=r, column=0, sticky="w", padx=4, pady=1)
            ttk.Entry(opts, textvariable=var, width=7).grid(row=r, column=1, sticky="w", padx=4)
        self.dose_multiphase = tk.BooleanVar(value=True)
        self.dose_history = tk.BooleanVar(value=True)
        self.dose_home_weigh = tk.BooleanVar(value=True)
        self.dose_init = tk.BooleanVar(value=True)
        checks = [("Multi-phase: volle slagen, wegen, rest (app)", self.dose_multiphase),
                  ("Adaptieve historie (app, alleen in geheugen)", self.dose_history),
                  ("Ook wegen na terugkeer naar nul", self.dose_home_weigh),
                  ("INIT + START vooraf", self.dose_init)]
        for i, (label, var) in enumerate(checks):
            ttk.Checkbutton(opts, text=label, variable=var).grid(row=len(fields) + i, column=0, columnspan=2,
                                                                 sticky="w", padx=2)
        run = ttk.Frame(opts)
        run.grid(row=len(fields) + len(checks), column=0, columnspan=2, sticky="w", padx=2, pady=4)
        self.dose_start_btn = ttk.Button(run, text="Start test", command=self.start_dose_test)
        self.dose_start_btn.pack(side="left", padx=2)
        self.dose_stop_btn = ttk.Button(run, text="Stop", command=self.cancel_dose, state="disabled")
        self.dose_stop_btn.pack(side="left", padx=2)
        report = ttk.Frame(opts)
        report.grid(row=len(fields) + len(checks) + 1, column=0, columnspan=2, sticky="w", padx=2)
        ttk.Button(report, text="Open rapport", command=self.open_report).pack(side="left", padx=2)
        ttk.Button(report, text="Map openen", command=lambda: self._open_dir(DOSE_DIR)).pack(side="left", padx=2)
        self.dose_progress = ttk.Progressbar(opts, maximum=100)
        self.dose_progress.grid(row=len(fields) + len(checks) + 2, column=0, columnspan=2, sticky="we", padx=4, pady=2)
        self.dose_status_lbl = ttk.Label(opts, text="", foreground="#555", wraplength=300)
        self.dose_status_lbl.grid(row=len(fields) + len(checks) + 3, column=0, columnspan=2, sticky="w", padx=4)

        res = ttk.LabelFrame(parent, text="Resultaten")
        res.pack(side="left", fill="both", expand=True, padx=4, pady=4)
        cols = [("idx", "#", 35), ("target", "Doel", 55), ("disp", "Gedoseerd", 80), ("err", "Fout g", 65),
                ("pct", "Fout %", 60), ("total", "Incl. terug", 80), ("stable", "Stabiel", 55), ("dur", "Duur s", 55)]
        self.dose_tree = ttk.Treeview(res, columns=[c[0] for c in cols], show="headings", height=7)
        for key, text, width in cols:
            self.dose_tree.heading(key, text=text)
            self.dose_tree.column(key, width=width, anchor="e", stretch=False)
        self.dose_tree.tag_configure("bad", foreground="#c01c28")
        self.dose_tree.pack(fill="x", padx=4, pady=2)
        scols = [("target", "Doel", 55), ("n", "n", 35), ("mean", "Gem. g", 75), ("sd", "SD g", 70),
                 ("cv", "CV %", 55), ("err", "Gem. fout %", 80), ("max", "Max |fout| %", 85)]
        self.summary_tree = ttk.Treeview(res, columns=[c[0] for c in scols], show="headings", height=4)
        for key, text, width in scols:
            self.summary_tree.heading(key, text=text)
            self.summary_tree.column(key, width=width, anchor="e", stretch=False)
        self.summary_tree.pack(fill="x", padx=4, pady=2)
        self._plan_refresh()

    def _build_forward_only_ui(self, parent):
        """Calibrate with scale → learn history → production open-loop (scale check-only)."""
        info = ttk.LabelFrame(parent, text="Forward-only doseerpomp")
        info.pack(fill="x", padx=4, pady=4)
        ttk.Label(
            info,
            text=(
                "1) Kalibreren: prime + anker → vaste g/omw.\n"
                "2) Inleren / doseren: mik iets onder doel, daarna vooruit bijsturen tot exact\n"
                "   het gevraagde gewicht (weegschaal). Geen afval, geen reverse.\n"
                "3) Zonder weegschaal: alleen open-loop schatting (minder nauwkeurig).\n"
                "Node/motor uit tab Pomp."
            ),
            justify="left", foreground="#333",
        ).pack(anchor="w", padx=6, pady=4)

        cal = ttk.LabelFrame(parent, text="1. Kalibratie")
        cal.pack(fill="x", padx=4, pady=2)
        self.fo_steps_rev = tk.StringVar(value=str(fo.DEFAULT_PUMP_STEPS_PER_REV))
        self.fo_gpr = tk.StringVar(value=str(fo.DEFAULT_G_PER_PUMP_REV))
        self.fo_prime_n = tk.StringVar(value=str(fo.PRIME_ROTATIONS))
        self.fo_anchor_n = tk.StringVar(value=str(fo.DEFAULT_ANCHOR_N))
        self.fo_fluid = tk.StringVar(value="water")
        self.fo_init = tk.BooleanVar(value=True)
        row = ttk.Frame(cal)
        row.pack(fill="x", padx=6, pady=4)
        for label, var, w in [
            ("Stappen/omw", self.fo_steps_rev, 7),
            ("g/omw", self.fo_gpr, 8),
            ("Prime", self.fo_prime_n, 3),
            ("Anker", self.fo_anchor_n, 3),
            ("Vloeistof", self.fo_fluid, 8),
        ]:
            ttk.Label(row, text=label).pack(side="left", padx=(0, 2))
            ttk.Entry(row, textvariable=var, width=w).pack(side="left", padx=(0, 8))
        ttk.Checkbutton(row, text="INIT+START", variable=self.fo_init).pack(side="left", padx=4)
        btns = ttk.Frame(cal)
        btns.pack(fill="x", padx=6, pady=(0, 4))
        self.fo_cal_btn = ttk.Button(btns, text="Kalibreren", command=self.fo_start_calibrate)
        self.fo_cal_btn.pack(side="left", padx=2)
        ttk.Button(btns, text="Laden…", command=self.fo_load_cal).pack(side="left", padx=2)
        ttk.Button(btns, text="Profiel laden", command=self.fo_load_profile).pack(side="left", padx=2)
        self.fo_cal_lbl = ttk.Label(cal, text="Nog niet gekalibreerd — standaard g/omw.", foreground="#555")
        self.fo_cal_lbl.pack(anchor="w", padx=6, pady=(0, 4))

        learn = ttk.LabelFrame(parent, text="2. Inleerproces (automatisch, met weegschaal)")
        learn.pack(fill="x", padx=4, pady=2)
        self.fo_learn_plan = tk.StringVar(value=fo.DEFAULT_LEARN_PLAN)
        self.fo_learn_pause = tk.StringVar(value=str(fo.DEFAULT_LEARN_PAUSE_S))
        self.fo_learn_streak = tk.StringVar(value=str(fo.DEFAULT_LEARN_STREAK))
        self.fo_tol = tk.StringVar(value=str(fo.DEFAULT_TOLERANCE_PCT))
        lrow = ttk.Frame(learn)
        lrow.pack(fill="x", padx=6, pady=6)
        ttk.Label(lrow, text="Gewichten (g)").pack(side="left")
        ttk.Entry(lrow, textvariable=self.fo_learn_plan, width=28).pack(side="left", padx=6)
        ttk.Label(lrow, text="OK op rij").pack(side="left", padx=(8, 2))
        ttk.Entry(lrow, textvariable=self.fo_learn_streak, width=3).pack(side="left")
        ttk.Label(lrow, text="Tol %").pack(side="left", padx=(8, 2))
        ttk.Entry(lrow, textvariable=self.fo_tol, width=5).pack(side="left")
        ttk.Label(lrow, text="Pauze s").pack(side="left", padx=(8, 2))
        ttk.Entry(lrow, textvariable=self.fo_learn_pause, width=5).pack(side="left")
        self.fo_learn_btn = ttk.Button(lrow, text="Start inleren", command=self.fo_start_learn)
        self.fo_learn_btn.pack(side="left", padx=10)
        ttk.Button(lrow, text="Inleren laden…", command=self.fo_load_learn).pack(side="left", padx=4)
        ttk.Label(
            learn,
            text="Na geslaagd inleren: profiel wordt opgeslagen (g/omw + yield). Daarna Doseer of 'Profiel laden'.",
            foreground="gray",
        ).pack(anchor="w", padx=6, pady=(0, 4))
        self.fo_hist_lbl = ttk.Label(learn, text="Historie: —", foreground="#555")
        self.fo_hist_lbl.pack(anchor="w", padx=6, pady=(0, 4))

        dose = ttk.LabelFrame(parent, text="3. Productie (basis + trim, weegschaal)")
        dose.pack(fill="x", padx=4, pady=2)
        self.fo_target = tk.StringVar(value="15")
        drow = ttk.Frame(dose)
        drow.pack(fill="x", padx=6, pady=8)
        ttk.Label(drow, text="Gewicht (g)", font=("Segoe UI", 11)).pack(side="left", padx=(0, 8))
        self.fo_target_entry = ttk.Entry(drow, textvariable=self.fo_target, width=10, font=("Consolas", 14))
        self.fo_target_entry.pack(side="left", padx=(0, 12))
        self.fo_target_entry.bind("<Return>", lambda e: self.fo_start_dose())
        self.fo_dose_btn = ttk.Button(drow, text="Doseer", command=self.fo_start_dose)
        self.fo_dose_btn.pack(side="left", padx=4)
        self.fo_stop_btn = ttk.Button(drow, text="Stop", command=self.cancel_forward_only, state="disabled")
        self.fo_stop_btn.pack(side="left", padx=4)
        ttk.Label(
            dose,
            text="Met weegschaal verplicht: zelfde pad als inleren (basis + micro-trim). Eindtrim max ~480 stappen.",
            foreground="gray",
        ).pack(anchor="w", padx=6, pady=(0, 6))

        testrow = ttk.Frame(dose)
        testrow.pack(fill="x", padx=6, pady=4)
        self.fo_no_trim_targets = tk.StringVar(value="1; 5; 10")
        self.fo_no_trim_repeats = tk.StringVar(value="5")
        ttk.Label(testrow, text="Zonder trim: doelen (g)").pack(side="left")
        ttk.Entry(testrow, textvariable=self.fo_no_trim_targets, width=20).pack(side="left", padx=4)
        ttk.Label(testrow, text="Herhalingen").pack(side="left")
        ttk.Entry(testrow, textvariable=self.fo_no_trim_repeats, width=4).pack(side="left", padx=4)
        self.fo_no_trim_btn = ttk.Button(testrow, text="Test zonder trim", command=self.fo_start_no_trim_test)
        self.fo_no_trim_btn.pack(side="left", padx=4)
        ttk.Label(dose, text="Zonder trim: kalibratie g/omw → absolute doelpositie; schaal controleert alleen. Geen yield-correctie of bijleren.", foreground="gray").pack(anchor="w", padx=6)

        mid = ttk.Frame(parent)
        mid.pack(fill="both", expand=True, padx=4, pady=2)
        st = ttk.LabelFrame(mid, text="Status")
        st.pack(side="left", fill="y", padx=(0, 4))
        self.fo_progress = ttk.Progressbar(st, maximum=100, length=260)
        self.fo_progress.pack(fill="x", padx=6, pady=6)
        self.fo_status_lbl = ttk.Label(
            st, text="Eerst kalibreren, daarna Start inleren of Doseer.",
            foreground="#555", wraplength=280,
        )
        self.fo_status_lbl.pack(anchor="w", padx=6, pady=2)
        self.fo_primed_lbl = ttk.Label(st, text="Priming: nog niet", foreground="#c88800")
        self.fo_primed_lbl.pack(anchor="w", padx=6, pady=(0, 6))

        res = ttk.LabelFrame(mid, text="Resultaten")
        res.pack(side="left", fill="both", expand=True)
        cols = [
            ("idx", "#", 40), ("kind", "Soort", 70), ("target", "Gevraagd", 70),
            ("disp", "Gemeten", 75), ("pct", "Fout %", 60), ("steps", "Stappen", 70),
            ("gpr", "g/omw", 60), ("dur", "s", 45),
        ]
        self.fo_tree = ttk.Treeview(res, columns=[c[0] for c in cols], show="headings", height=8)
        for key, text, width in cols:
            self.fo_tree.heading(key, text=text)
            self.fo_tree.column(key, width=width, anchor="e", stretch=False)
        self.fo_tree.tag_configure("bad", foreground="#c01c28")
        self.fo_tree.tag_configure("ok", foreground="#26a269")
        self.fo_tree.pack(fill="both", expand=True, padx=4, pady=2)
        ttk.Button(res, text="Map openen", command=lambda: self._open_dir(FO_DIR)).pack(anchor="w", padx=4, pady=2)

    def _refresh_com_ports(self):
        self.scale_port_box["values"] = [p.device for p in serial.tools.list_ports.comports()]

    def _update_hint(self):
        try:
            self.pos_hint.config(text=f"= {int(self.position.get()) / STEPS_PER_REV:.3f} omw")
        except ValueError:
            self.pos_hint.config(text="")

    # ---------- connection ----------

    def toggle_connect(self):
        if self.bus:
            self.disconnect()
        else:
            self.connect()

    def connect(self):
        try:
            bitrate = int(self.bitrate.get())
            if self.iface.get() == "virtual":
                self.bus = can.Bus(interface="virtual", channel="pump_debug", receive_own_messages=False)
            else:
                self.bus = can.Bus(interface="ixxat", channel=int(self.channel.get()), bitrate=bitrate,
                                   rx_fifo_size=1024)
        except Exception as exc:
            self.bus = None
            messagebox.showerror("Verbinden mislukt",
                                 f"{exc}\n\nIs de backend nog met CAN verbonden? Verbreek die eerst.")
            return
        self.running = True
        self.stats = {"TX": 0, "RX": 0, "ERR": 0, "HB": 0}
        self.last_heartbeat.clear()
        self._open_session_log()
        self.rx_thread = threading.Thread(target=self._rx_loop, daemon=True)
        self.rx_thread.start()
        self.connect_btn.config(text="Verbreken")
        self.status_lbl.config(text=f"Verbonden {self.iface.get()} ch{self.channel.get()} @ {bitrate}",
                               foreground="green")
        self.fo_primed = False
        self.fo_phase = 0
        self.fo_base_yield = 1.0
        if hasattr(self, "fo_primed_lbl"):
            self._fo_update_primed_lbl()
        self._event("INFO", None, b"", f"Verbonden: {self.iface.get()} kanaal {self.channel.get()} "
                                       f"{bitrate} bps - log: {self.csv_path.name}")

    def disconnect(self):
        self.cancel_calibration()
        self.cancel_dose()
        self.cancel_forward_only()
        self.running = False
        if self.rx_thread:
            self.rx_thread.join(timeout=1.0)
        try:
            if self.bus:
                self.bus.shutdown()
        except Exception as exc:
            self._event("ERR", None, b"", f"Shutdown: {exc}")
        self.bus = None
        self.connect_btn.config(text="Verbinden")
        self.status_lbl.config(text="Niet verbonden", foreground="gray")
        self._event("INFO", None, b"", "Verbinding verbroken")
        if self.csv_file:
            self.csv_file.close()
            self.csv_file = None

    def _open_session_log(self):
        LOG_DIR.mkdir(parents=True, exist_ok=True)
        self.csv_path = LOG_DIR / f"pump_debug_{datetime.now():%Y%m%d_%H%M%S}.csv"
        self.csv_file = open(self.csv_path, "w", newline="", encoding="utf-8")
        self.csv_writer = csv.writer(self.csv_file)
        self.csv_writer.writerow(["time", "dir", "id_dec", "id_hex", "data", "decode"])

    def _rx_loop(self):
        while self.running and self.bus:
            try:
                msg = self.bus.recv(timeout=0.1)
            except Exception as exc:  # e.g. VCIError on bus-off / warning limit
                self._event("ERR", None, b"", f"recv: {exc}")
                time.sleep(0.1)
                continue
            if msg is None:
                continue
            if msg.is_error_frame:
                self._event("ERR", msg.arbitration_id, bytes(msg.data), "error frame")
            else:
                self._track_feedback(msg.arbitration_id, bytes(msg.data))
                self._event("RX", msg.arbitration_id, bytes(msg.data))

    def _track_feedback(self, node: int, data: bytes):
        """Record reported positions and release a calibration move waiting for its done frame."""
        feedback = parse_position_feedback(data)
        if not feedback:
            return
        motor, position, is_done = feedback
        self.reported[(node, motor)] = position
        if is_done and self.move_wait == (node, motor):
            self.move_done_pos = position
            self.move_done.set()

    # ---------- sending ----------

    def _send_frame(self, can_id: int, data: bytes) -> bool:
        if not self.bus:
            self._event("ERR", None, b"", "Niet verbonden")
            return False
        try:
            self.bus.send(can.Message(arbitration_id=can_id, data=data, is_extended_id=False), timeout=0.5)
            self._event("TX", can_id, data)
            return True
        except Exception as exc:
            self._event("ERR", can_id, data, f"send: {exc}")
            return False

    def _sequence(self, node: int, motor: int, cmd: int, position: int, speed: int, accel: int, decel: int):
        """VWA 8-frame command: rec 8..2 parameters (cmd=0), rec 1 executes."""
        position = max(position, 0)
        params = {8: 0, 7: 0, 6: decel, 5: accel, 4: speed,
                  3: position % STEPS_PER_REV, 2: position // STEPS_PER_REV}
        with self.send_lock:
            for rec in range(8, 1, -1):
                frame = bytes([0, rec, motor, 0xBB]) + int(params[rec]).to_bytes(4, "little", signed=True)
                if not self._send_frame(node, frame):
                    return False
                time.sleep(FRAME_GAP_S)
            frame = bytes([cmd, 1, motor, 0xBB]) + int(position).to_bytes(4, "little", signed=True)
            return self._send_frame(node, frame)

    def _read_params(self):
        try:
            node, motor = int(self.node.get()), int(self.motor.get())
            vals = [int(v.get()) for v in (self.position, self.speed, self.accel, self.decel)]
        except ValueError:
            messagebox.showerror("Ongeldige invoer", "Node, motor en waarden moeten gehele getallen zijn.")
            return None
        if not 1 <= motor <= 8:
            messagebox.showerror("Ongeldige invoer", "Motor moet 1-8 zijn.")
            return None
        return node, motor, *vals

    def _run(self, label: str, fn):
        """Run a TX job in a worker so the UI stays responsive."""
        if not self.bus:
            messagebox.showwarning("Niet verbonden", "Eerst verbinden.")
            return
        self._event("INFO", None, b"", f">> {label}")
        threading.Thread(target=fn, daemon=True).start()

    def command(self, name: str, position=None):
        p = self._read_params()
        if not p:
            return
        node, motor, pos, speed, accel, decel = p
        pos = pos if position is None else position
        if name not in ("MOVE_TO", "MOVE_TO_ZERO"):
            pos = 0
        if name == "ABORT":
            speed = accel = 0

        def job():
            if self._sequence(node, motor, COMMANDS[name], pos, speed, accel, decel) and name in ("MOVE_TO_ZERO", "ZERO"):
                self.commanded[motor] = 0

        self._run(f"{name} node {node} motor {motor}" + (f" pos {pos}" if name.startswith("MOVE") else ""), job)

    def init_start(self):
        p = self._read_params()
        if not p:
            return
        node, motor, _, speed, accel, decel = p
        self._run(f"INIT + START node {node} motor {motor}",
                  lambda: self._init_start_seq(node, motor, speed, accel, decel))

    def _init_start_seq(self, node, motor, speed, accel, decel) -> bool:
        if not self._sequence(node, motor, COMMANDS["INIT"], 0, speed, accel, decel):
            return False
        time.sleep(1.0)  # drive setup before START
        ok = self._sequence(node, motor, COMMANDS["START"], 0, speed, accel, decel)
        time.sleep(0.5)  # firmware drive-on delay
        return ok

    def move_to(self, target=None):
        p = self._read_params()
        if not p:
            return
        node, motor, pos, speed, accel, decel = p
        pos = pos if target is None else target
        if pos < 0:
            messagebox.showerror("Ongeldige positie", "Positie kan niet negatief zijn.")
            return
        eta = pos and abs(pos - self.commanded[motor]) / max(speed, 1)

        def job():
            if self._sequence(node, motor, COMMANDS["MOVE_TO"], pos, speed, accel, decel):
                self.commanded[motor] = pos

        self._run(f"MOVE_TO node {node} motor {motor} -> {pos} ({pos / STEPS_PER_REV:.3f} omw, "
                  f"~{eta:.1f}s zonder ramp)", job)

    def jog_move(self, direction: int):
        p = self._read_params()
        if not p:
            return
        try:
            steps = int(float(self.jog.get()) * STEPS_PER_REV)
        except ValueError:
            return
        motor = p[1]
        self.move_to(max(self.commanded[motor] + direction * steps, 0))

    def send_custom(self):
        p = self._read_params()
        if not p:
            return
        try:
            cmd = int(self.custom_cmd.get(), 16)
        except ValueError:
            messagebox.showerror("Ongeldig", "Commando als hex, bijv. 4A")
            return
        node, motor, pos, speed, accel, decel = p
        self._run(f"Custom cmd 0x{cmd:02X} node {node} motor {motor} pos {pos}",
                  lambda: self._sequence(node, motor, cmd, pos, speed, accel, decel))

    def send_raw(self):
        try:
            text = self.raw_id.get().strip()
            can_id = int(text, 16) if text.lower().startswith("0x") else int(text)
            data = bytes.fromhex(self.raw_data.get().replace(",", " "))
        except ValueError:
            messagebox.showerror("Ongeldig frame", "ID decimaal of 0x.., data als hex bytes.")
            return
        if len(data) > 8:
            messagebox.showerror("Ongeldig frame", "Maximaal 8 databytes.")
            return
        self._run(f"Raw frame {can_id}", lambda: self._send_frame(can_id, data))

    def abort_all(self):
        self.cancel_calibration()
        self.cancel_dose()
        self.cancel_forward_only()
        if not self.bus:
            return
        try:
            node = int(self.node.get())
        except ValueError:
            node = 610

        def job():
            for motor in range(1, 9):
                self._sequence(node, motor, COMMANDS["ABORT"], 0, 0, 0, 51200)

        self._run(f"ESC: ABORT alle motoren node {node}", job)

    # ---------- scale ----------

    def toggle_scale(self):
        if self.scale:
            self.disconnect_scale()
        else:
            self.connect_scale()

    def connect_scale(self):
        tcp = self.scale_mode.get() == "TCP"
        host, port = self.scale_host.get().strip(), self.scale_port.get().strip()
        try:
            baud, tcp_port = int(self.scale_baud.get()), int(self.scale_tcp_port.get())
        except ValueError:
            messagebox.showerror("Ongeldige invoer", "Baud en TCP-poort moeten gehele getallen zijn.")
            return
        if (tcp and not host) or (not tcp and not port):
            messagebox.showerror("Ongeldige invoer", "Vul een TCP host of COM-poort in.")
            return
        label = f"tcp://{host}:{tcp_port}" if tcp else f"{port} @ {baud}"
        self.scale_btn.config(state="disabled")
        self.scale_status_lbl.config(text=f"Verbinden met {label}…", foreground="gray")

        def job():
            scale = ScaleCommunicator()
            try:
                ok = scale.connect_tcp(host, tcp_port) if tcp else scale.connect(port, baud)
                error = "" if ok else "Geen verbinding (poort bezet? backend nog verbonden?)"
            except Exception as exc:
                ok, error = False, str(exc)
            self._ui(lambda: self._scale_connected(scale if ok else None, label, error))

        threading.Thread(target=job, daemon=True).start()

    def _scale_connected(self, scale, label, error):
        self.scale_btn.config(state="normal")
        if not scale:
            self.scale_status_lbl.config(text="Niet verbonden", foreground="gray")
            self._event("ERR", None, b"", f"Weegschaal {label}: {error}")
            messagebox.showerror("Weegschaal", f"{label}\n\n{error}")
            return
        self.scale = scale
        self.scale_running = True
        self.scale_thread = threading.Thread(target=self._scale_poll_loop, daemon=True)
        self.scale_thread.start()
        self.scale_btn.config(text="Verbreken")
        self.scale_status_lbl.config(text=f"Verbonden {label}", foreground="green")
        self._event("SCALE", None, b"", f"Weegschaal verbonden: {label}")

    def disconnect_scale(self):
        self.cancel_calibration()
        self.cancel_dose()
        self.cancel_forward_only()
        self.scale_running = False
        if self.scale_thread:
            self.scale_thread.join(timeout=2.0)
        scale, self.scale = self.scale, None
        if scale:
            with self.scale_lock:
                try:
                    scale.disconnect()
                except Exception as exc:
                    self._event("ERR", None, b"", f"Weegschaal verbreken: {exc}")
        self.scale_reading = None
        self.scale_btn.config(text="Verbinden")
        self.scale_status_lbl.config(text="Niet verbonden", foreground="gray")
        self._event("SCALE", None, b"", "Weegschaal verbroken")

    def _scale_poll_loop(self):
        while self.scale_running:
            # Reason: during an activity the worker reads the scale itself; skipping keeps its timing undisturbed.
            if self.scale_live_on and not self.busy:
                self.read_scale()
            time.sleep(SCALE_POLL_S)

    def read_scale(self):
        """Thread-safe single SI read; updates the live display. Returns WeightReading or None."""
        scale = self.scale
        if not scale:
            return None
        with self.scale_lock:
            try:
                reading = scale.get_reading()
            except Exception as exc:
                self._event("ERR", None, b"", f"Weegschaal lezen: {exc}")
                return None
        if reading:
            # Reason: the shared parser falls back to 'generic_simple' for 'S D' lines and marks them stable.
            status = mtsics_stable(reading.raw_data)
            self.scale_status_known = status is not None
            if status is not None:
                reading.stable = status
            self.scale_reading, self.scale_reading_at = reading, time.monotonic()
            if self.scale_log_on:
                self._event("SCALE", None, b"", f"{reading.weight:.4f} {reading.unit} "
                                                f"{'stabiel' if reading.stable else 'onstabiel'}")
        return reading

    def scale_action(self, action: str):
        if not self.scale:
            messagebox.showwarning("Weegschaal", "Eerst de weegschaal verbinden.")
            return
        if self.busy and action != "read":
            messagebox.showwarning("Weegschaal", f"Niet tijdens {self.busy}.")
            return

        def job():
            if action == "read":
                reading = self.read_scale()
                text = (f"SI: {reading.weight:.4f} {reading.unit} {'stabiel' if reading.stable else 'onstabiel'} "
                        f"raw={reading.raw_data.strip()!r}") if reading else "SI: geen antwoord"
                self._event("SCALE", None, b"", text)
                return
            with self.scale_lock:
                ok = self.scale.tare() if action == "tare" else self.scale.zero_scale()
            self._event("SCALE" if ok else "ERR", None, b"",
                        f"{'Tarra (ZI)' if action == 'tare' else 'Nul (Z)'}: {'ok' if ok else 'mislukt'}")

        threading.Thread(target=job, daemon=True).start()

    # ---------- calibration ----------

    def start_calibration(self):
        if self.busy:
            messagebox.showwarning("Kalibratie", f"Er loopt al een {self.busy}.")
            return
        if not self.bus:
            messagebox.showwarning("Kalibratie", "Eerst CAN verbinden.")
            return
        if not self.scale:
            messagebox.showwarning("Kalibratie", "Eerst de weegschaal verbinden.")
            return
        p = self._read_params()
        if not p:
            return
        node, motor, _, speed, accel, decel = p
        try:
            samples, runs, angle = int(self.cal_samples.get()), int(self.cal_runs.get()), float(self.cal_angle.get())
        except ValueError:
            messagebox.showerror("Ongeldige invoer", "Samples/runs geheel getal, hoek een getal.")
            return
        if samples < 1 or runs < 1 or not 0 < angle <= 360 or speed <= 0:
            messagebox.showerror("Ongeldige invoer", "Samples en runs >= 1, hoek 0-360°, snelheid > 0.")
            return
        fluid = self.cal_fluid.get().strip() or "water"
        reverse = self.cal_reverse.get()
        if not messagebox.askyesno(
                "Kalibratie starten",
                f"Node {node} motor {motor}, vloeistof '{fluid}'\n"
                f"{runs} run(s) × {samples + 1} punten tot {angle:g}°"
                f"{', met terugslag naar nul' if reverse else ''}.\n\n"
                "Beker op de weegschaal en slang gevuld? Esc annuleert."):
            return

        self.cal_cancel.clear()
        self.cal_ramp = (accel, decel)
        self.cal_wait_done = self.cal_wait.get()
        ctrl = VWACalibrationController(
            node_id=node, motor_id=motor, fluid_name=fluid,
            move_motor_callback=self._cal_move, get_weight_callback=self._cal_weight,
            is_stable_callback=self._cal_stable, stop_motor_callback=self._cal_stop,
            max_samples=samples, max_angle=angle, num_runs=runs, use_reverse=reverse,
            steps_per_revolution=STEPS_PER_REV, is_simulated=self.iface.get() == "virtual",
            speed=speed, acceleration=accel, deceleration=decel,
        )
        ctrl.set_progress_callback(lambda status: self._ui(lambda: self._cal_progress(status)))
        self.calibration = ctrl
        self.busy = "kalibratie"
        self.cal_start_btn.config(state="disabled")
        self.cal_cancel_btn.config(state="normal")
        self.cal_progress["value"] = 0
        self.cal_result_lbl.config(text="")
        self._event("INFO", None, b"", f">> KALIBRATIE node {node} motor {motor} '{fluid}': {runs} runs, "
                                       f"{samples} samples, {angle:g}°, reverse={reverse}, "
                                       f"speed={speed} accel={accel} decel={decel}")
        init_first = self.cal_init.get()
        threading.Thread(target=self._cal_worker, args=(ctrl, init_first, speed), daemon=True).start()

    def _cal_worker(self, ctrl, init_first, speed):
        result, error = None, ""
        try:
            accel, decel = self.cal_ramp
            if init_first and not self._init_start_seq(ctrl.node_id, ctrl.motor_id, speed, accel, decel):
                error = "INIT + START mislukt"
            else:
                result = asyncio.run(ctrl.run_calibration())
        except Exception as exc:
            error = str(exc)
        self._ui(lambda: self._cal_finished(result, error))

    def _move_and_wait(self, node, motor, steps, speed, accel, decel, cancel, wait_done=True, tag="CAL"):
        """One absolute MOVE_TO (MOVE_TO_ZERO for 0); if wait_done, block until the board's done frame.

        Returns True when the done frame arrived. Raises RuntimeError when sending fails.
        """
        cmd = COMMANDS["MOVE_TO_ZERO"] if steps == 0 else COMMANDS["MOVE_TO"]
        distance = steps - self.commanded[motor]
        self.move_done.clear()
        self.move_wait = (node, motor)
        started = time.monotonic()
        if not self._sequence(node, motor, cmd, steps, speed, accel, decel):
            self.move_wait = None
            raise RuntimeError(f"CAN verzenden mislukt (doel {steps})")
        self.commanded[motor] = steps
        if not wait_done:
            self.move_wait = None
            return False
        timeout = move_timeout(distance, speed, accel, decel)
        deadline = started + timeout
        while not self.move_done.wait(0.05):
            if cancel.is_set() or time.monotonic() > deadline:
                break
        self.move_wait = None
        if self.move_done.is_set():
            self._event(tag, None, b"", f"   beweging klaar na {time.monotonic() - started:.2f}s: "
                                        f"doel {steps}, gemeld {self.move_done_pos} "
                                        f"(afwijking {self.move_done_pos - steps:+d})")
            return True
        if not cancel.is_set():
            self._event("ERR", None, b"", f"Geen MOVE_TO_DONE binnen {timeout:.1f}s (doel {steps})")
        return False

    def _cal_move(self, node, motor, steps, speed):
        """Controller callback: one VWA move; optionally block until the board reports it done."""
        accel, decel = self.cal_ramp
        self._move_and_wait(node, motor, steps, speed, accel, decel, self.cal_cancel, self.cal_wait_done)

    def _cal_weight(self):
        reading = self.read_scale()
        if not reading or not math.isfinite(reading.weight):
            raise RuntimeError("Geen geldige weegschaalwaarde tijdens kalibratie")
        return reading.weight

    def _cal_stable(self):
        reading = self.read_scale()
        return bool(reading and reading.stable)

    def _cal_stop(self, node, motor):
        decel = self.cal_ramp[1]
        threading.Thread(target=lambda: self._sequence(node, motor, COMMANDS["STOP"], 0, 0, 0, decel),
                         daemon=True).start()

    def cancel_calibration(self):
        ctrl = self.calibration
        if not ctrl or self.cal_cancel.is_set():
            return
        self.cal_cancel.set()
        self._event("INFO", None, b"", ">> KALIBRATIE annuleren")
        ctrl.cancel()

    def _cal_progress(self, status):
        self.cal_progress["value"] = status.get("progress", 0)
        self.cal_status_lbl.config(text=status.get("message", ""), foreground="#555")

    def _cal_finished(self, result, error):
        self.calibration = None
        self.busy = None
        self.cal_start_btn.config(state="normal")
        self.cal_cancel_btn.config(state="disabled")
        if not result or not result.is_valid:
            message = error or (result.error_message if result else "onbekende fout")
            self.cal_status_lbl.config(text=f"Kalibratie mislukt: {message}", foreground="#c01c28")
            self._event("ERR", None, b"", f"KALIBRATIE mislukt: {message}")
            return
        self.cal_progress["value"] = 100
        saved = self._save_calibration(result)
        summary = (f"dVsF {result.dVsF / 1000:.4f} g  dVsR {result.dVsR / 1000:.4f} g  "
                   f"max pos {result.max_position:.4f} omw  {len(result.calibration_table)} punten")
        self.cal_result_lbl.config(text=summary)
        self.cal_status_lbl.config(text=f"Klaar - opgeslagen als {saved.name}" if saved else
                                   "Klaar - opslaan mislukt, zie log", foreground="green" if saved else "#c01c28")
        self._event("INFO", None, b"", f"KALIBRATIE klaar: {summary}")
        self._set_active_cal(result, f"nieuwe kalibratie ({saved.name if saved else 'niet opgeslagen'})")

    def _save_calibration(self, result):
        """Write unified-format JSON (same shape as data/calibrations) + per-run CSV. Returns JSON path."""
        CAL_DIR.mkdir(parents=True, exist_ok=True)
        stem = f"{result.pump_id}_{safe_name(result.fluid_name)}_{datetime.now():%Y%m%d_%H%M%S}"
        now = datetime.now().isoformat()
        payload = {
            "pump_id": result.pump_id, "node_id": result.node_id, "motor_id": result.motor_id,
            "fluid_name": result.fluid_name, "created_at": now, "last_updated": now,
            "steps_per_revolution": result.steps_per_revolution,
            "vwa_calibration": result.to_dict(), "stroke_stabilization": None,
            "notes": "pump_debug tool",
        }
        try:
            json_path = CAL_DIR / f"{stem}.json"
            json_path.write_text(json.dumps(payload, indent=2), encoding="utf-8")
            with open(CAL_DIR / f"{stem}.csv", "w", newline="", encoding="utf-8") as f:
                writer = csv.writer(f)
                writer.writerow(["run", "index", "angle_deg", "steps", "position_rev",
                                 "dispensedF_mg", "dispensedR_mg", "dispensed_mg"])
                writer.writerows(calibration_csv_rows(result))
        except OSError as exc:
            self._event("ERR", None, b"", f"Kalibratie opslaan: {exc}")
            return None
        self._event("INFO", None, b"", f"Kalibratie opgeslagen: {json_path}")
        return json_path

    def open_cal_dir(self):
        self._open_dir(CAL_DIR)

    def _open_dir(self, path: Path):
        path.mkdir(parents=True, exist_ok=True)
        self._open_path(path)

    def _open_path(self, path: Path):
        try:
            os.startfile(path)
        except (AttributeError, OSError) as exc:
            messagebox.showinfo("Openen", f"{path}\n\n{exc}")

    # ---------- active calibration ----------

    def _set_active_cal(self, cal, source: str):
        self.active_cal = cal
        self.active_cal_src = source
        self.active_cal_lbl.config(text=(
            f"{cal.pump_id} / {cal.fluid_name}   bron: {source}\n"
            f"dVsF {cal.dVsF / 1000:.4f} g/slag   dVsR {cal.dVsR / 1000:.4f} g   "
            f"slag {cal.max_position:.4f} omw ({cal.get_full_stroke_steps()} stappen)\n"
            f"{len(cal.calibration_table)} tabelpunten, {len(cal.dispense_history)} historie-items, "
            f"datum {cal.calibration_date[:19]}{'  (GESIMULEERD)' if cal.is_simulated else ''}"))
        self._event("INFO", None, b"", f"Actieve kalibratie: {cal.pump_id}/{cal.fluid_name} ({source}), "
                                       f"dVsF {cal.dVsF:.1f} mg")

    def load_cal_from_db(self):
        try:
            node, motor = int(self.node.get()), int(self.motor.get())
        except ValueError:
            messagebox.showerror("Ongeldige invoer", "Node en motor moeten gehele getallen zijn.")
            return
        pump_id, fluid = f"node_{node}_motor_{motor}", self.cal_fluid.get().strip()
        try:
            cal, fluids = dosing.load_calibration_from_db(APP_DB, pump_id, fluid)
        except (sqlite3.Error, ValueError, OSError) as exc:
            messagebox.showerror("App-database", f"{APP_DB}\n\n{exc}")
            return
        if not cal:
            available = f"Beschikbaar voor deze pomp: {', '.join(fluids)}" if fluids else "Geen kalibraties voor deze pomp."
            messagebox.showwarning("App-database", f"Geen kalibratie voor {pump_id} / '{fluid}'.\n{available}")
            return
        if not cal.has_valid_calibration_table():
            messagebox.showerror("App-database", f"Kalibratie {pump_id} / '{fluid}' is ongeldig of leeg.")
            return
        self._set_active_cal(cal, "app-database")

    def load_cal_from_file(self):
        start_dir = CAL_DIR if CAL_DIR.exists() else REPO_ROOT / "data" / "calibrations"
        path = filedialog.askopenfilename(initialdir=start_dir, filetypes=[("JSON", "*.json")])
        if not path:
            return
        try:
            cal = dosing.calibration_from_payload(json.loads(Path(path).read_text(encoding="utf-8")))
        except (OSError, ValueError) as exc:
            messagebox.showerror("Kalibratie laden", f"{path}\n\n{exc}")
            return
        if not cal.has_valid_calibration_table():
            messagebox.showerror("Kalibratie laden", "Kalibratie is ongeldig of heeft te weinig tabelpunten.")
            return
        self._set_active_cal(cal, Path(path).name)

    # ---------- dosing (single weight + dose test) ----------

    def _dose_setting_vars(self):
        return {"pause": self.dose_pause, "stable_timeout": self.dose_stable_timeout,
                "stable_hold": self.dose_stable_hold, "multiphase": self.dose_multiphase,
                "history": self.dose_history, "home_weigh": self.dose_home_weigh, "init": self.dose_init}

    def _dose_options(self):
        try:
            opts = {"pause": float(self.dose_pause.get()), "stable_timeout": float(self.dose_stable_timeout.get()),
                    "stable_hold": float(self.dose_stable_hold.get())}
        except ValueError:
            messagebox.showerror("Ongeldige invoer", "Pauze en stabiliteitstijden moeten getallen zijn.")
            return None
        if min(opts.values()) < 0:
            messagebox.showerror("Ongeldige invoer", "Tijden kunnen niet negatief zijn.")
            return None
        opts.update(multiphase=self.dose_multiphase.get(), history=self.dose_history.get(),
                    home_weigh=self.dose_home_weigh.get(), init=self.dose_init.get())
        return opts

    def _dose_preflight(self):
        """Shared checks for a single dose and the dose test. Returns (params, opts) or None."""
        if self.busy:
            messagebox.showwarning("Doseren", f"Er loopt al een {self.busy}.")
            return None
        if not self.bus or not self.scale:
            messagebox.showwarning("Doseren", "Eerst CAN én weegschaal verbinden.")
            return None
        if not self.active_cal:
            messagebox.showwarning("Doseren", "Eerst een kalibratie laden of uitvoeren (tab Kalibratie & doseren).")
            return None
        p = self._read_params()
        if not p:
            return None
        node, motor, _, speed, accel, decel = p
        if speed <= 0:
            messagebox.showerror("Ongeldige invoer", "Snelheid moet > 0 zijn.")
            return None
        cal = self.active_cal
        if (cal.node_id, cal.motor_id) != (node, motor) and not messagebox.askyesno(
                "Andere pomp", f"De kalibratie is van {cal.pump_id}, maar 'Pomp' staat op node {node} "
                               f"motor {motor}. Toch doseren?"):
            return None
        opts = self._dose_options()
        if not opts:
            return None
        return (node, motor, speed, accel, decel), opts

    def _check_cancel(self):
        if self.dose_cancel.is_set():
            raise Cancelled()

    def _sleep(self, seconds: float):
        if self.dose_cancel.wait(seconds):
            raise Cancelled()

    def _stable_weight(self, opts):
        """Wait until the scale reports 'S S' continuously for stable_hold s (max stable_timeout s).

        Returns (reading or None, became_stable, waited_s); on timeout the last reading is returned.
        """
        start, since, last = time.monotonic(), None, None
        while True:
            self._check_cancel()
            reading = self.read_scale()
            now = time.monotonic()
            if reading and math.isfinite(reading.weight):
                last = reading
                if reading.stable:
                    since = now if since is None else since
                    if now - since >= opts["stable_hold"]:
                        return reading, True, now - start
                else:
                    since = None
            if now - start >= opts["stable_timeout"]:
                return last, False, now - start
            time.sleep(0.1)

    def _dose_move(self, params, steps, warnings):
        node, motor, speed, accel, decel = params
        self._check_cancel()
        if not self._move_and_wait(node, motor, steps, speed, accel, decel, self.dose_cancel, True, tag="DOSE"):
            self._check_cancel()
            warnings.append(f"geen MOVE_TO_DONE (doel {steps})")

    def _dose_start_sequence(self, params, opts):
        """Optional INIT + START, then MOVE_TO_ZERO: the app assumes every dose starts at position 0."""
        node, motor, speed, accel, decel = params
        if opts["init"] and not self._init_start_seq(node, motor, speed, accel, decel):
            raise RuntimeError("INIT + START mislukt")
        self._dose_move(params, 0, [])

    @staticmethod
    def _empty_record(target):
        record = dict.fromkeys(dosing.RECORD_FIELDS)
        record.update(target_g=target, stable_after=False, warnings="", error="")
        return record

    def _dose(self, base_cal, target, params, opts):
        """One dose following the backend hardware path (_dose_fluid), measured with real stability.

        Multi-phase: full strokes, weigh, average dVsF with the measured stroke, then the partial for the
        remainder, then MOVE_TO_ZERO. Works on a copy; with history on, the result is added to base_cal.
        """
        cal = copy.deepcopy(base_cal)
        rec, warnings = self._empty_record(target), []
        started = time.monotonic()
        revs, steps = cal.calculate_dose_steps(target)
        if steps <= 0:
            raise RuntimeError(f"{target:g} g geeft 0 stappen")
        before, stable0, _ = self._stable_weight(opts)
        if not before:
            raise RuntimeError("Geen weegschaalwaarde vóór de dosering")
        if not stable0:
            warnings.append("start niet stabiel")
        w0 = before.weight

        full = int(math.floor(target * 1000.0 / cal.dVsF))
        method = "single"
        if opts["multiphase"] and full >= 1:
            method = f"multi-phase ({full} slagen)"
            phase_steps = full * cal.get_full_stroke_steps()
            self._dose_move(params, phase_steps, warnings)
            mid, _, _ = self._stable_weight(opts)
            if mid:
                so_far = mid.weight - w0
                rec["mid_dispensed_g"] = so_far
                if so_far > 0:
                    cal.dVsF = (cal.dVsF + so_far * 1000.0 / full) / 2.0
                steps = phase_steps
                remaining = target - so_far
                if remaining > 0.001:
                    _, partial = cal.calculate_partial_for_remaining(remaining)
                    if partial > 0:
                        steps = phase_steps + partial
                        self._dose_move(params, steps, warnings)
            else:
                warnings.append("geen tussenweging, rest volgens berekening")
                self._dose_move(params, steps, warnings)
            revs = steps / cal.steps_per_revolution
        else:
            self._dose_move(params, steps, warnings)

        after, stable1, settle = self._stable_weight(opts)
        if not after:
            raise RuntimeError("Geen weegschaalwaarde na de dosering")
        if not stable1:
            warnings.append(f"niet stabiel na {opts['stable_timeout']:g} s")
        dispensed = after.weight - w0
        if opts["history"] and dispensed > 0:
            base_cal.add_to_history(dispensed, revs)

        self._dose_move(params, 0, warnings)
        if opts["home_weigh"]:
            home, _, _ = self._stable_weight(opts)
            if home:
                rec["weight_home_g"] = home.weight
                rec["dispensed_total_g"] = home.weight - w0
                rec["error_total_pct"] = dosing.dose_error(target, rec["dispensed_total_g"])[1]

        error_g, error_pct = dosing.dose_error(target, dispensed)
        rec.update(method=method, steps=steps, revolutions=round(revs, 5), weight_before_g=w0,
                   weight_after_g=after.weight, dispensed_g=dispensed, error_g=error_g, error_pct=error_pct,
                   stable_after=stable1, settle_s=settle, duration_s=time.monotonic() - started,
                   warnings="; ".join(warnings))
        total = rec["dispensed_total_g"]
        self._event("DOSE", None, b"", f"DOSIS {target:g} g: {dispensed:.4f} g ({error_pct:+.2f}%)"
                                       + (f", incl. terug {total:.4f} g" if total is not None else "")
                                       + f", {steps} stappen, {method}"
                                       + (f" - {rec['warnings']}" if warnings else ""))
        return rec

    def dose_single(self):
        try:
            target = float(self.single_target.get().replace(",", "."))
        except ValueError:
            messagebox.showerror("Ongeldige invoer", "Gewicht moet een getal zijn.")
            return
        if target <= 0:
            messagebox.showerror("Ongeldige invoer", "Gewicht moet > 0 zijn.")
            return
        pre = self._dose_preflight()
        if not pre:
            return
        params, opts = pre
        self.busy, self.dose_params = "dosering", params
        self.dose_cancel.clear()
        self.single_result_lbl.config(text=f"Doseren {target:g} g…", foreground="#555")
        self._event("DOSE", None, b"", f">> DOSEER {target:g} g met {self.active_cal.pump_id}/"
                                       f"{self.active_cal.fluid_name}")
        cal = self.active_cal

        def job():
            record, error = None, ""
            try:
                self._dose_start_sequence(params, opts)
                record = self._dose(cal, target, params, opts)
            except Cancelled:
                error = "gestopt door gebruiker"
            except Exception as exc:
                error = str(exc)
            self._ui(lambda: self._single_finished(record, error))

        threading.Thread(target=job, daemon=True).start()

    def _single_finished(self, rec, error):
        self.busy = None
        if error or not rec:
            self.single_result_lbl.config(text=f"Dosering mislukt: {error}", foreground="#c01c28")
            self._event("ERR", None, b"", f"DOSERING mislukt: {error}")
            return
        total = rec["dispensed_total_g"]
        self.single_result_lbl.config(
            text=(f"Doel {rec['target_g']:g} g -> {rec['dispensed_g']:.4f} g "
                  f"({rec['error_g']:+.4f} g, {rec['error_pct']:+.2f}%)"
                  + (f"\nincl. terugkeer naar nul: {total:.4f} g" if total is not None else "")
                  + f"\n{rec['steps']} stappen, {rec['method']}, stabiel: {'ja' if rec['stable_after'] else 'NEE'}"
                  + (f"\n{rec['warnings']}" if rec["warnings"] else "")),
            foreground="#222" if rec["stable_after"] else "#c88800")

    def start_dose_test(self):
        if not self.plan:
            messagebox.showwarning("Doseertest", "Het testplan is leeg.")
            return
        pre = self._dose_preflight()
        if not pre:
            return
        params, opts = pre
        cal = self.active_cal
        doses = dosing.expand_plan(self.plan)
        total_g = sum(target for _, _, target in doses)
        if not messagebox.askyesno(
                "Doseertest starten",
                f"{len(doses)} doseringen ({dosing.plan_to_text(self.plan)}), totaal {total_g:g} g\n"
                f"met {cal.pump_id} / {cal.fluid_name}.\n\nPast dit in de beker op de weegschaal? "
                "Stop of Esc breekt af; het rapport wordt ook dan gemaakt."):
            return
        self.busy, self.dose_params = "doseertest", params
        self.dose_cancel.clear()
        self.dose_records = []
        self.last_report = None
        self.dose_tree.delete(*self.dose_tree.get_children())
        self.summary_tree.delete(*self.summary_tree.get_children())
        self.dose_progress["value"] = 0
        self.dose_start_btn.config(state="disabled")
        self.dose_stop_btn.config(state="normal")
        meta = self._report_meta(cal, params, opts)
        self._event("DOSE", None, b"", f">> DOSEERTEST {dosing.plan_to_text(self.plan)} met "
                                       f"{cal.pump_id}/{cal.fluid_name}")
        # Reason: history from this test accumulates on a copy, so the active calibration stays as loaded.
        threading.Thread(target=self._dose_test_worker,
                         args=(copy.deepcopy(cal), doses, params, opts, meta, datetime.now()), daemon=True).start()

    def _dose_test_worker(self, cal, doses, params, opts, meta, started_at):
        error = ""
        try:
            self._dose_start_sequence(params, opts)
            for i, (group, rep, target) in enumerate(doses, 1):
                self._check_cancel()
                self._ui(lambda i=i, target=target: self._dose_test_progress(i, len(doses), target))
                try:
                    rec = self._dose(cal, target, params, opts)
                except Cancelled:
                    raise
                except Exception as exc:
                    rec = self._empty_record(target)
                    rec["error"] = str(exc)
                    self._event("ERR", None, b"", f"Dosis {i} ({target:g} g): {exc}")
                    self._dose_move(params, 0, [])  # next dose must start from position 0 again
                rec.update(index=i, group=group, repeat=rep)
                self._ui(lambda rec=rec: self._dose_test_row(rec))
                if i < len(doses):
                    self._sleep(opts["pause"])
        except Cancelled:
            error = "gestopt door gebruiker"
        except Exception as exc:
            error = str(exc)
        self._ui(lambda: self._dose_test_finished(cal, meta, started_at, error))

    def _dose_test_progress(self, i, n, target):
        self.dose_progress["value"] = (i - 1) / n * 100
        self.dose_status_lbl.config(text=f"Dosis {i}/{n}: {target:g} g…", foreground="#555")

    def _dose_test_row(self, rec):
        self.dose_records.append(rec)

        def fmt(value, digits=4):
            return "" if value is None else f"{value:.{digits}f}"

        bad = bool(rec["error"]) or not rec["stable_after"]
        item = self.dose_tree.insert("", "end", tags=("bad",) if bad else (), values=(
            rec["index"], f"{rec['target_g']:g}", fmt(rec["dispensed_g"]), fmt(rec["error_g"]),
            fmt(rec["error_pct"], 2), fmt(rec["dispensed_total_g"]), "ja" if rec["stable_after"] else "nee",
            fmt(rec["duration_s"], 1)))
        self.dose_tree.see(item)
        self.summary_tree.delete(*self.summary_tree.get_children())
        for g in dosing.summarize(self.dose_records):
            d = g["dispensed"] or {}
            self.summary_tree.insert("", "end", values=(
                f"{g['target_g']:g}", f"{g['n']}" + (f" (+{g['failed']} fout)" if g["failed"] else ""),
                fmt(d.get("mean")), fmt(d.get("sd")), fmt(d.get("cv_pct"), 2),
                fmt(g["mean_error_pct"], 2), fmt(g["max_abs_error_pct"], 2)))

    def _dose_test_finished(self, cal, meta, started_at, error):
        self.busy = None
        self.dose_start_btn.config(state="normal")
        self.dose_stop_btn.config(state="disabled")
        if not error:
            self.dose_progress["value"] = 100
        meta = meta + [("Start", f"{started_at:%Y-%m-%d %H:%M:%S}"), ("Einde", f"{datetime.now():%Y-%m-%d %H:%M:%S}"),
                       ("Resultaat", error or "voltooid"), ("Doseringen uitgevoerd", len(self.dose_records))]
        report = ""
        if self.dose_records:
            stem = DOSE_DIR / f"doseertest_{cal.pump_id}_{safe_name(cal.fluid_name)}_{started_at:%Y%m%d_%H%M%S}"
            try:
                self.last_report = dosing.write_report(stem, meta, self.dose_records)
                report = f" - rapport {self.last_report.name}"
                self._event("INFO", None, b"", f"Doseertest rapport: {self.last_report}")
            except OSError as exc:
                self._event("ERR", None, b"", f"Rapport opslaan: {exc}")
                report = " - rapport opslaan mislukt"
        self.dose_status_lbl.config(text=(f"Gestopt: {error}" if error else "Klaar") + report,
                                    foreground="#c01c28" if error else "green")
        self._event("DOSE", None, b"", f"DOSEERTEST {error or 'klaar'}: {len(self.dose_records)} doseringen{report}")

    def _report_meta(self, cal, params, opts):
        node, motor, speed, accel, decel = params
        return [
            ("Pomp", f"node {node} motor {motor}"), ("Vloeistof", cal.fluid_name),
            ("Kalibratie", f"{cal.pump_id} - bron: {self.active_cal_src}, datum {cal.calibration_date[:19]}"
                           f"{' (GESIMULEERD)' if cal.is_simulated else ''}"),
            ("dVsF / dVsR", f"{cal.dVsF:.1f} mg / {cal.dVsR:.1f} mg per slag"),
            ("Slag", f"{cal.max_position:.4f} omw = {cal.get_full_stroke_steps()} stappen"),
            ("Testplan", dosing.plan_to_text(self.plan)),
            ("Snelheid / accel / decel", f"{speed} / {accel} / {decel} stap/s"),
            ("Methode", "multi-phase: volle slagen, tussenweging, rest (zoals app)" if opts["multiphase"]
             else "één beweging naar berekende positie"),
            ("Adaptieve historie", "aan (in geheugen, zoals app)" if opts["history"] else "uit"),
            ("Stabiel", f"weegschaal 'S S' {opts['stable_hold']:g} s aanhouden, max {opts['stable_timeout']:g} s"),
            ("Pauze tussen doses", f"{opts['pause']:g} s"),
            ("Weegschaal", self.scale_status_lbl.cget("text")),
        ]

    def cancel_dose(self):
        if self.busy not in ("dosering", "doseertest") or self.dose_cancel.is_set():
            return
        self.dose_cancel.set()
        node, motor, _, _, decel = self.dose_params
        self._event("INFO", None, b"", ">> DOSERING stoppen")
        threading.Thread(target=lambda: self._sequence(node, motor, COMMANDS["STOP"], 0, 0, 0, decel),
                         daemon=True).start()

    # ---------- forward-only program (prime / dose / set-zero) ----------

    def _fo_read_cfg(self):
        p = self._read_params()
        if not p:
            return None
        node, motor, _, speed, accel, decel = p
        try:
            cfg = {
                "node": node, "motor": motor, "speed": speed, "accel": accel, "decel": decel,
                "steps_rev": int(self.fo_steps_rev.get()),
                "gpr": float(self.fo_gpr.get().replace(",", ".")),
                "prime_n": int(self.fo_prime_n.get()),
                "anchor_n": int(self.fo_anchor_n.get()),
                "target": float(self.fo_target.get().replace(",", ".")),
                "tol": float(self.fo_tol.get().replace(",", ".")),
                "init": bool(self.fo_init.get()),
                "fluid": (self.fo_fluid.get() or "water").strip(),
                "learn_pause": float(self.fo_learn_pause.get().replace(",", ".")),
                "learn_streak": int(self.fo_learn_streak.get()),
                "undercut": fo.DEFAULT_UNDERCUT,
                "max_trim": fo.DEFAULT_MAX_TRIM,
            }
        except ValueError:
            messagebox.showerror("Forward-only", "Parameters moeten geldige getallen zijn.")
            return None
        if cfg["steps_rev"] <= 0 or cfg["gpr"] <= 0 or cfg["prime_n"] < 1 or cfg["anchor_n"] < 1:
            messagebox.showerror("Forward-only", "stappen/omw, g/omw, prime en anker moeten > 0.")
            return None
        if cfg["learn_streak"] < 1:
            messagebox.showerror("Forward-only", "OK op rij moet ≥ 1 zijn.")
            return None
        return cfg

    def _fo_check_cancel(self):
        if self.fo_cancel.is_set():
            raise Cancelled("forward-only geannuleerd")

    def _fo_move_forward(self, node, motor, absolute, speed, accel, decel):
        self._fo_check_cancel()
        if absolute < 0:
            raise RuntimeError(f"REFUSED negatieve positie {absolute}")
        commanded = self.commanded[motor]
        if absolute < commanded:
            raise RuntimeError(f"REFUSED MOVE_TO {absolute} < commanded {commanded}")
        if absolute == commanded:
            return True
        distance = absolute - commanded
        self.move_done.clear()
        self.move_wait = (node, motor)
        started = time.monotonic()
        if not self._sequence(node, motor, COMMANDS["MOVE_TO"], absolute, speed, accel, decel):
            self.move_wait = None
            raise RuntimeError(f"CAN MOVE_TO {absolute} mislukt")
        self.commanded[motor] = absolute
        timeout = move_timeout(distance, speed, accel, decel, margin=5.0)
        deadline = started + timeout
        while not self.move_done.wait(0.05):
            if self.fo_cancel.is_set() or time.monotonic() > deadline:
                break
        self.move_wait = None
        if self.move_done.is_set():
            return True
        reported = self.reported.get((node, motor), self.move_done_pos)
        if reported is not None and abs(reported - absolute) <= 64:
            return True
        if self.fo_cancel.is_set():
            raise Cancelled("forward-only geannuleerd")
        raise RuntimeError(f"Geen MOVE_TO_DONE binnen {timeout:.1f}s (doel {absolute})")

    def _fo_set_zero(self, node, motor, decel=0):
        self._fo_check_cancel()
        self.reported[(node, motor)] = None
        ok = self._sequence(node, motor, COMMANDS["ZERO"], 0, 0, 0, decel)
        time.sleep(0.25)
        deadline = time.monotonic() + 0.8
        reported = self.reported.get((node, motor))
        while time.monotonic() < deadline:
            reported = self.reported.get((node, motor))
            if reported is not None and abs(reported) <= 64:
                break
            time.sleep(0.05)
        self.commanded[motor] = 0
        self.reported[(node, motor)] = 0 if reported is None else reported
        if not ok:
            raise RuntimeError("PLC_ZERO mislukt — geweigerd door te gaan")
        return True

    def _fo_stable(self, hold=0.8, timeout=12.0):
        start, since, last = time.monotonic(), None, None
        while True:
            self._fo_check_cancel()
            reading = self.read_scale()
            now = time.monotonic()
            if reading and math.isfinite(reading.weight):
                last = reading
                if reading.stable:
                    if since is None:
                        since = now
                    elif now - since >= hold:
                        return reading, True, now - start
                else:
                    since = None
            if now - start >= timeout:
                return last, False, now - start
            time.sleep(0.1)

    def _fo_set_busy(self, busy: bool):
        state = "normal" if not busy else "disabled"
        self.fo_cal_btn.config(state=state)
        self.fo_learn_btn.config(state=state)
        self.fo_dose_btn.config(state=state)
        self.fo_no_trim_btn.config(state=state)
        self.fo_stop_btn.config(state="disabled" if not busy else "normal")
        self.fo_target_entry.config(state=state)

    def _fo_update_primed_lbl(self):
        if self.fo_primed:
            self.fo_primed_lbl.config(text="Priming: gedaan", foreground="#26a269")
        else:
            self.fo_primed_lbl.config(text="Priming: nog niet", foreground="#c88800")

    def _fo_update_hist_lbl(self):
        self.fo_hist_lbl.config(text="Historie: " + fo.history_summary(self.fo_history))

    def fo_start_calibrate(self):
        cfg = self._fo_begin("forward-only kalibratie", need_scale=True)
        if not cfg:
            return
        threading.Thread(target=self._fo_calibrate_worker, args=(cfg,), daemon=True).start()

    def fo_load_cal(self):
        path = filedialog.askopenfilename(
            title="Forward-only kalibratie / inleerprofiel laden",
            initialdir=str(FO_DIR) if FO_DIR.exists() else str(LOG_DIR),
            filetypes=[("JSON", "*.json"), ("Alle bestanden", "*.*")],
        )
        if not path:
            return
        self._fo_load_profile_path(Path(path))

    def fo_load_learn(self):
        """Load a learn_fo_*.json via file picker."""
        initial = str(FO_DIR) if FO_DIR.exists() else str(LOG_DIR)
        path = filedialog.askopenfilename(
            title="Inleerprofiel laden",
            initialdir=initial,
            filetypes=[
                ("Inleer JSON", "learn_fo_*.json"),
                ("Profiel", "fo_profile_*.json"),
                ("JSON", "*.json"),
                ("Alle bestanden", "*.*"),
            ],
        )
        if not path:
            return
        self._fo_load_profile_path(Path(path))

    def fo_load_profile(self):
        """Load stable fo_profile_{node}_m{motor}.json for the selected pump."""
        try:
            node, motor = int(self.node.get()), int(self.motor.get())
        except ValueError:
            messagebox.showerror("Profiel", "Node/motor ongeldig (tab Pomp).")
            return
        path = FO_DIR / f"fo_profile_{node}_m{motor}.json"
        if not path.exists():
            cands = sorted(FO_DIR.glob(f"learn_fo_{node}_m{motor}_*.json"), reverse=True)
            if not cands:
                messagebox.showwarning(
                    "Profiel",
                    f"Geen opgeslagen inleerprofiel voor node {node} motor {motor}.\n"
                    "Eerst inleren of 'Inleren laden…' gebruiken.",
                )
                return
            path = cands[0]
        self._fo_load_profile_path(path)

    def _fo_load_profile_path(self, path: Path):
        try:
            payload = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError) as exc:
            messagebox.showerror("Laden", str(exc))
            return
        gpr = None
        # Working gpr from learn (not the flawed finalize diagnostic).
        for key in ("g_per_pump_rev_working", "g_per_pump_rev_start"):
            if payload.get(key):
                try:
                    gpr = float(payload[key])
                    break
                except (TypeError, ValueError):
                    pass
        if gpr is None:
            gpr = fo.load_gpr(payload)
        if gpr is None:
            messagebox.showerror("Laden", "Geen geldige g_per_pump_rev in dit bestand.")
            return
        self._fo_apply_profile(payload, gpr, path)

    def _fo_apply_profile(self, payload: dict, gpr: float, path: Path):
        steps = int(payload.get("pump_steps_per_rev") or fo.DEFAULT_PUMP_STEPS_PER_REV)
        self.fo_gpr.set(f"{gpr:.5f}")
        self.fo_steps_rev.set(str(steps))
        if payload.get("fluid"):
            self.fo_fluid.set(str(payload["fluid"]))
        if payload.get("tolerance_pct") is not None:
            self.fo_tol.set(str(payload["tolerance_pct"]))
        if payload.get("plan"):
            self.fo_learn_plan.set(str(payload["plan"]))
        if payload.get("streak") is not None:
            self.fo_learn_streak.set(str(payload["streak"]))

        by = payload.get("base_yield_ema")
        if by is None:
            hist = payload.get("history") or []
            if hist:
                by = hist[-1].get("base_yield_ema")
        try:
            self.fo_base_yield = float(by) if by is not None else 1.0
        except (TypeError, ValueError):
            self.fo_base_yield = 1.0

        hist = payload.get("history") or []
        if hist:
            self.fo_history = list(hist)
            for item in self.fo_tree.get_children():
                self.fo_tree.delete(item)
            for i, rec in enumerate(self.fo_history[-40:], start=1):
                if rec.get("kind") == "dose":
                    self._fo_row({**rec, "index": i})
            self._fo_update_hist_lbl()

        self.fo_cal = payload
        self.fo_cal_path = path
        mode = payload.get("mode", "")
        kind = "inleren" if "learn" in str(mode) else ("live" if "live" in str(mode) else "kalibratie")
        self.fo_cal_lbl.config(
            text=(
                f"Profiel ({kind}): {path.name}  →  g/omw={gpr:.4f}  "
                f"yield={self.fo_base_yield:.3f}"
            ),
            foreground="#26a269",
        )
        self.fo_status_lbl.config(
            text=f"Profiel geladen — klaar om te doseren (yield {self.fo_base_yield:.3f}).",
            foreground="green",
        )
        self._event(
            "FO", None, b"",
            f"Profiel geladen: {path} gpr={gpr:.4f} yield={self.fo_base_yield:.3f} mode={mode}",
        )

    def _fo_save_learn_profile(self, cfg, payload: dict) -> Path:
        """Write timestamped learn file + stable fo_profile for this pump."""
        FO_DIR.mkdir(parents=True, exist_ok=True)
        stamp = payload.get("started") or datetime.now().strftime("%Y%m%d_%H%M%S")
        path = FO_DIR / f"learn_fo_{cfg['node']}_m{cfg['motor']}_{stamp}.json"
        text = json.dumps(payload, indent=2)
        path.write_text(text, encoding="utf-8")
        profile = FO_DIR / f"fo_profile_{cfg['node']}_m{cfg['motor']}.json"
        profile.write_text(text, encoding="utf-8")
        live = {
            "mode": "forward_only_live",
            "node": cfg["node"], "motor": cfg["motor"], "fluid": cfg["fluid"],
            "pump_steps_per_rev": cfg["steps_rev"],
            "g_per_pump_rev": payload.get("g_per_pump_rev_working") or payload.get("g_per_pump_rev"),
            "base_yield_ema": payload.get("base_yield_ema", self.fo_base_yield),
            "history": payload.get("history") or self.fo_history[-50:],
            "learn_file": path.name,
            "updated": stamp,
        }
        (FO_DIR / f"fo_live_{cfg['node']}_m{cfg['motor']}.json").write_text(
            json.dumps(live, indent=2), encoding="utf-8",
        )
        return path

    def fo_start_learn(self):
        """Run learn: each target until consecutive OK doses within tolerance."""
        cfg = self._fo_begin("forward-only inleren", need_scale=True)
        if not cfg:
            return
        try:
            targets = fo.parse_learn_targets(self.fo_learn_plan.get())
        except ValueError as exc:
            self.busy = None
            self._fo_set_busy(False)
            messagebox.showerror("Inleren", str(exc))
            return
        if not targets:
            self.busy = None
            self._fo_set_busy(False)
            messagebox.showerror("Inleren", "Lege inleerreeks — vul bijv. 5 of 5,10,15 in.")
            return
        threading.Thread(target=self._fo_learn_worker, args=(cfg, targets), daemon=True).start()

    def fo_start_dose(self):
        """Single production dose — same closed-loop path as inleren (scale required)."""
        cfg = self._fo_begin("forward-only productie", need_scale=True)
        if not cfg:
            return
        if cfg["target"] <= 0:
            self.busy = None
            self._fo_set_busy(False)
            messagebox.showerror("Forward-only", "Vul een gewicht > 0 in.")
            return
        threading.Thread(target=self._fo_dose_worker, args=(cfg,), daemon=True).start()

    def fo_start_no_trim_test(self):
        try:
            targets = fo.parse_learn_targets(self.fo_no_trim_targets.get())
            repeats = int(self.fo_no_trim_repeats.get())
            if not targets or not 1 <= repeats <= 100 or len(targets) * repeats > 1000:
                raise ValueError("Kies 1–100 herhalingen, maximaal 1000 doses")
            import math
            if any(not math.isfinite(t) or t <= 0 for t in targets):
                raise ValueError("Doelen moeten positief en eindig zijn")
        except ValueError as exc:
            messagebox.showerror("Test zonder trim", str(exc))
            return
        cfg = self._fo_begin("forward-only test zonder trim", need_scale=True)
        if not cfg:
            return
        cfg = dict(cfg, trim_enabled=False)
        threading.Thread(target=self._fo_no_trim_worker, args=(cfg, targets, repeats), daemon=True).start()

    def _fo_no_trim_worker(self, cfg, targets, repeats):
        """Fixed calibration only: one forward move per dose, scale verifies."""
        error = ""
        records = []
        path = None
        try:
            import math
            if not math.isfinite(cfg["gpr"]) or cfg["gpr"] <= 0 or cfg["steps_rev"] <= 0:
                raise ValueError("Geldige kalibratie g/omw en stappen/omw vereist")
            self._fo_ensure_ready(cfg)
            FO_DIR.mkdir(parents=True, exist_ok=True)
            path = FO_DIR / ("fo_no_trim_" + datetime.now().strftime("%Y%m%d_%H%M%S_%f"))
            for target in targets:
                for repeat in range(1, repeats + 1):
                    self._fo_check_cancel()
                    attempt = {"target_g": target, "repeat": repeat, "status": "running",
                               "start_position_steps": self.commanded[cfg["motor"]]}
                    records.append(attempt)
                    self._fo_save_no_trim(path, cfg, records, "running")
                    try:
                        rec = self._fo_one_dose(cfg, target, "test zonder trim")
                        attempt.update(rec, status="completed")
                        attempt["within_tolerance"] = fo.within_tolerance(rec["dispensed_g"], target, cfg["tol"])
                    except Exception as exc:
                        attempt.update(status="failed", error=str(exc), end_position_steps=self.commanded[cfg["motor"]])
                        raise
                    finally:
                        self._fo_save_no_trim(path, cfg, records, "running")
        except Exception as exc:
            error = str(exc)
            self._event("ERR", None, b"", "Test zonder trim: " + error)
            self.cancel_forward_only()
        finally:
            try:
                if path:
                    self._fo_save_no_trim(path, cfg, records, "failed" if error else "completed", error)
            finally:
                self._ui(lambda: self._fo_finished(error, "test zonder trim"))

    def _fo_save_no_trim(self, path, cfg, records, status, error=""):
        payload = {"mode": "calibration_only_no_trim", "status": status, "error": error,
                   "context": cfg, "records": records, "interpretation": "Gemeten uitstroom; geen directe plunjerverplaatsing"}
        path.with_suffix(".json").write_text(json.dumps(payload, indent=2), encoding="utf-8")
        keys = sorted({key for row in records for key in row})
        with path.with_suffix(".csv").open("w", newline="", encoding="utf-8-sig") as stream:
            writer = csv.DictWriter(stream, fieldnames=keys)
            writer.writeheader()
            writer.writerows(records)

    def _fo_begin(self, label: str, need_scale: bool = True):
        if self.busy:
            messagebox.showwarning("Forward-only", f"Er loopt al een {self.busy}.")
            return None
        if not self.bus:
            messagebox.showwarning("Forward-only", "Eerst CAN verbinden.")
            return None
        if need_scale and not self.scale:
            messagebox.showwarning("Forward-only", "Eerst weegschaal verbinden.")
            return None
        cfg = self._fo_read_cfg()
        if not cfg:
            return None
        self.fo_cancel.clear()
        self.busy = label
        self._fo_set_busy(True)
        self.fo_progress["value"] = 0
        self.fo_status_lbl.config(text=f"Bezig: {label}…", foreground="#555")
        self._event("FO", None, b"", f">> {label.upper()} node {cfg['node']} motor {cfg['motor']}")
        return cfg

    def _fo_run_prime(self, cfg):
        node, motor = cfg["node"], cfg["motor"]
        steps = cfg["prime_n"] * cfg["steps_rev"]
        self._ui(lambda: self.fo_status_lbl.config(text=f"Priming {cfg['prime_n']} omw…"))
        # Continuous stroke then set-zero only.
        if self.scale:
            self._fo_stable(hold=0.3)
        self._fo_move_forward(node, motor, self.commanded[motor] + steps,
                              cfg["speed"], cfg["accel"], cfg["decel"])
        self._fo_set_zero(node, motor, cfg["decel"])
        self.fo_primed = True
        self.fo_phase = 0
        self._ui(self._fo_update_primed_lbl)
        self._event("FO", None, b"", f"PRIME {cfg['prime_n']}×{cfg['steps_rev']} klaar")

    def _fo_calibrate_worker(self, cfg):
        error = ""
        try:
            node, motor = cfg["node"], cfg["motor"]
            steps_rev = cfg["steps_rev"]
            if cfg["init"] and not self._init_start_seq(node, motor, cfg["speed"], cfg["accel"], cfg["decel"]):
                raise RuntimeError("INIT + START mislukt")
            self._fo_set_zero(node, motor, cfg["decel"])
            self._fo_run_prime(cfg)

            anchor = []
            n = cfg["anchor_n"]
            for i in range(1, n + 1):
                self._fo_check_cancel()
                self._ui(lambda i=i, n=n: (
                    self.fo_progress.configure(value=100 * i / n),
                    self.fo_status_lbl.config(text=f"Anker {i}/{n}…"),
                ))
                before, _, _ = self._fo_stable(hold=0.8)
                if not before:
                    raise RuntimeError("Geen weegschaal vóór anker")
                w0 = before.weight
                self._fo_move_forward(node, motor, self.commanded[motor] + steps_rev,
                                      cfg["speed"], cfg["accel"], cfg["decel"])
                after, ok1, settle = self._fo_stable(hold=1.0)
                if not after:
                    raise RuntimeError("Geen weegschaal na anker")
                dg = after.weight - w0
                anchor.append({"i": i, "g": dg, "settle_s": settle, "stable": ok1})
                self._event("FO", None, b"", f"Anker {i}/{n}: {dg:+.4f} g")
                self._ui(lambda i=i, dg=dg, gpr=cfg["gpr"]: self._fo_row({
                    "index": f"A{i}", "kind": "anker", "target_g": None,
                    "dispensed_g": dg, "error_pct": None, "steps": steps_rev,
                    "gpr": None, "duration_s": None,
                }))
                self._fo_set_zero(node, motor, cfg["decel"])
                time.sleep(0.3)

            ag = [a["g"] for a in anchor]
            gpr = statistics.fmean(ag)
            sd = statistics.stdev(ag) if len(ag) > 1 else 0.0
            cv = (sd / gpr * 100) if gpr else None
            stamp = datetime.now().strftime("%Y%m%d_%H%M%S")
            result = {
                "mode": "forward_only_anchor",
                "pump_id": f"node_{node}_motor_{motor}",
                "node": node, "motor": motor, "fluid": cfg["fluid"],
                "started": stamp,
                "pump_steps_per_rev": steps_rev,
                "prime_rotations": cfg["prime_n"],
                "anchor_n": n, "anchor": anchor,
                "g_per_pump_rev": gpr,
                "anchor_sd_g": sd, "anchor_cv_pct": cv,
                "history": self.fo_history,
            }
            FO_DIR.mkdir(parents=True, exist_ok=True)
            path = FO_DIR / f"cal_fo_{node}_m{motor}_{stamp}.json"
            path.write_text(json.dumps(result, indent=2), encoding="utf-8")
            self.fo_cal, self.fo_cal_path = result, path

            def _apply():
                self.fo_gpr.set(f"{gpr:.5f}")
                warn = f"  CV {cv:.2f}%" if cv is not None else ""
                self.fo_cal_lbl.config(
                    text=f"g/omw={gpr:.4f} (sd={sd:.4f}{warn})  →  {path.name}",
                    foreground="#26a269" if (cv is None or cv <= 1.5) else "#c88800",
                )
                self.fo_status_lbl.config(
                    text="Kalibratie klaar — start inleren of doseer in productie.",
                    foreground="green",
                )

            self._ui(_apply)
            self._event("FO", None, b"", f"KALIBRATIE gpr={gpr:.4f} → {path.name}")
        except Cancelled:
            error = "geannuleerd"
        except Exception as exc:
            error = str(exc)
            self._event("ERR", None, b"", f"FO kalibratie: {exc}")
        self._ui(lambda: self._fo_finished(error, "kalibratie"))

    def _fo_ensure_ready(self, cfg):
        node, motor = cfg["node"], cfg["motor"]
        if cfg["init"] and not self.fo_primed:
            if not self._init_start_seq(node, motor, cfg["speed"], cfg["accel"], cfg["decel"]):
                raise RuntimeError("INIT + START mislukt")
        if not self.fo_primed:
            self._fo_run_prime(cfg)

    def _fo_one_dose(self, cfg, target_g: float, mode: str, progress_lo=20, progress_hi=80):
        """Dose any target grams. With scale: undercut + forward trim to exact (no waste)."""
        node, motor = cfg["node"], cfg["motor"]
        steps_rev = cfg["steps_rev"]
        gpr = cfg["gpr"]
        no_trim_test = cfg.get("trim_enabled") is False
        if no_trim_test and not self.scale:
            raise RuntimeError("Test zonder trim vereist weegschaalcontrole")
        use_trim = bool(self.scale) and not no_trim_test
        max_trim = int(cfg.get("max_trim", fo.DEFAULT_MAX_TRIM))

        if use_trim:
            plan = fo.plan_base_with_yield(
                target_g, gpr, steps_rev, base_yield=self.fo_base_yield,
            )
            label = (
                f"{mode}: {target_g:g} g  (basis→{plan['aim_g']:.3f} g, "
                f"yield {plan['base_yield']:.2f}, {plan['total_steps']} st)…"
            )
        else:
            plan = fo.plan_open_loop(target_g, gpr, steps_rev)
            label = f"{mode}: {target_g:g} g  ({plan['total_steps']} stappen, open-loop)…"
        self._ui(lambda: self.fo_status_lbl.config(text=label))
        self.fo_progress["value"] = progress_lo

        w0 = None
        if self.scale:
            before, _, _ = self._fo_stable(hold=0.5)
            w0 = before.weight if before else None
            if (use_trim or no_trim_test) and w0 is None:
                raise RuntimeError("Geen weegschaal vóór dosis")

        t0 = time.monotonic()
        total_steps = plan["total_steps"]
        cmd0 = self.commanded[motor]
        abs_target = cmd0 + total_steps
        self._event(
            "FO", None, b"",
            f"BASE {mode} doel={target_g:g}g aim={plan.get('aim_g', target_g):.4f}g "
            f"cmd_aim={plan.get('commanded_aim_g', plan.get('aim_g', target_g)):.4f}g "
            f"yield={plan.get('base_yield', 1):.3f} steps={total_steps} "
            f"abs={cmd0}→{abs_target} gpr={gpr:.5f}",
        )
        self._fo_move_forward(node, motor, abs_target, cfg["speed"], cfg["accel"], cfg["decel"])
        self.fo_progress["value"] = progress_hi

        dispensed = err_pct = None
        trim_n = 0
        trim_steps = 0
        trim_trace = []
        base_dispensed = None
        if no_trim_test and not self.scale:
            raise RuntimeError("Weegschaalverbinding verloren na dosis; resultaat ongeldig")
        if self.scale and w0 is not None:
            after, _, _ = self._fo_stable(hold=1.0)
            if not after:
                raise RuntimeError("Geen weegschaal na basisdosis")
            dispensed = after.weight - w0
            base_dispensed = dispensed
            aim_g = float(plan.get("aim_g") or target_g)
            if not no_trim_test:
                self.fo_base_yield = fo.update_base_yield(
                    self.fo_base_yield, aim_g, base_dispensed, target_g=target_g,
                )
            self._event(
                "FO", None, b"",
                f"BASE result {dispensed:.4f}g (aim {aim_g:.4f}, "
                f"{dispensed - target_g:+.4f} vs doel) "
                f"yield_ema={self.fo_base_yield:.3f} w0={w0:.4f} w1={after.weight:.4f}",
            )
            tol_g = target_g * (cfg["tol"] / 100.0)
            local_gpr = None
            last_dead = False
            dead_streak = 0
            while use_trim and trim_n < max_trim:
                self._fo_check_cancel()
                if fo.within_tolerance(dispensed, target_g, cfg["tol"]):
                    break
                need = target_g - dispensed
                if need <= 0:
                    break
                add = fo.trim_steps_for_need(
                    need, gpr, steps_rev,
                    tol_g=tol_g, local_gpr=local_gpr,
                    last_trim_dead=last_dead, dead_streak=dead_streak,
                    target_g=target_g,
                )
                if add <= 0:
                    break
                if fo.trim_would_risk_overshoot(need, add, gpr, steps_rev, tol_g):
                    add = min(add, fo.TRIM_CRAWL_STEPS)
                    if fo.trim_would_risk_overshoot(need, add, gpr, steps_rev, tol_g):
                        self._event(
                            "FO", None, b"",
                            f"TRIM stop onder doel need={need:.4f}g (overshoot-risico)",
                        )
                        break
                before_trim = dispensed
                cmd_before = self.commanded[motor]
                abs_trim = cmd_before + add
                kind = "escape" if (last_dead or dead_streak) else "trim"
                self._event(
                    "FO", None, b"",
                    f"{kind.upper()} #{trim_n + 1} need={need:.4f}g add={add} "
                    f"abs={cmd_before}→{abs_trim} local_gpr="
                    f"{local_gpr if local_gpr is not None else gpr:.5f} dead_streak={dead_streak}",
                )
                self._ui(lambda n=trim_n + 1, need=need, kind=kind: self.fo_status_lbl.config(
                    text=f"{mode}: {kind} {n}  (nog {need:.3f} g)…"))
                self._fo_move_forward(
                    node, motor, abs_trim, cfg["speed"], cfg["accel"], cfg["decel"],
                )
                trim_n += 1
                trim_steps += add
                total_steps += add
                after, _, _ = self._fo_stable(hold=0.8)
                if not after:
                    raise RuntimeError("Geen weegschaal na trim")
                dispensed = after.weight - w0
                got = dispensed - before_trim
                expected = add / steps_rev * gpr if steps_rev else 0
                last_dead = expected > 0 and got < expected * fo.TRIM_DEAD_RATIO
                dead_streak = (dead_streak + 1) if last_dead else 0
                accepted = fo.accept_local_gpr(got / (add / steps_rev), gpr) if got > 0 and add > 0 else None
                if accepted is not None:
                    local_gpr = accepted
                self._event(
                    "FO", None, b"",
                    f"{kind.upper()} #{trim_n} got={got:+.4f}g (exp {expected:.4f}) "
                    f"total={dispensed:.4f}g dead={last_dead} "
                    f"local_gpr={local_gpr if local_gpr is not None else '-'}",
                )
                trim_trace.append({
                    "n": trim_n, "kind": kind, "need_g": need, "add_steps": add,
                    "abs_from": cmd_before, "abs_to": abs_trim,
                    "got_g": got, "expected_g": expected, "dispensed_g": dispensed,
                    "dead": last_dead, "local_gpr": local_gpr,
                })
                if dispensed > target_g + tol_g:
                    self._event(
                        "FO", None, b"",
                        f"TRIM overshoot: {dispensed:.4f} g (doel {target_g:g})",
                    )
                    break
            _, err_pct = dosing.dose_error(target_g, dispensed)

        end_position = self.commanded[motor]
        if not no_trim_test:
            self._fo_set_zero(node, motor, cfg["decel"])
        self.fo_phase = end_position % steps_rev if no_trim_test else (total_steps % steps_rev if steps_rev else 0)

        dur = round(time.monotonic() - t0, 2)
        rec = {
            "kind": "dose", "mode": mode,
            "trim_enabled": use_trim, "calibration_only": no_trim_test,
            "start_position_steps": cmd0, "target_position_steps": abs_target,
            "end_position_steps": end_position,
            "target_g": target_g, "dispensed_g": dispensed,
            "error_pct": err_pct, "steps": total_steps,
            "base_steps": plan["total_steps"],
            "base_dispensed_g": base_dispensed,
            "base_aim_g": plan.get("aim_g"),
            "base_yield": plan.get("base_yield"),
            "base_yield_ema": self.fo_base_yield,
            "trim_n": trim_n, "trim_steps": trim_steps,
            "trim_trace": trim_trace,
            "steps_per_rev": steps_rev,
            "full_revs": total_steps // steps_rev if steps_rev else 0,
            "rem_steps": total_steps % steps_rev if steps_rev else 0,
            "undercut": plan.get("undercut") if use_trim else None,
            "gpr": gpr, "duration_s": dur,
        }
        self.fo_history.append(rec)
        self.fo_records.append(rec)
        idx = sum(1 for h in self.fo_history if h.get("kind") == "dose")
        self._ui(lambda: (self._fo_row({**rec, "index": idx}), self._fo_update_hist_lbl()))
        self._event(
            "FO", None, b"",
            f"DOSE done {mode} {target_g:g}g → "
            f"{dispensed if dispensed is not None else '?'} "
            f"steps={total_steps} trim_n={trim_n} {dur}s",
        )
        return rec

    def _fo_learn_worker(self, cfg, targets):
        """Per target: keep dosing until streak consecutive OK, then next target."""
        error = ""
        gpr_start = cfg["gpr"]
        learn_history = []
        target_results = []
        try:
            self._fo_ensure_ready(cfg)
            pause = max(0.0, cfg.get("learn_pause", fo.DEFAULT_LEARN_PAUSE_S))
            streak = int(cfg.get("learn_streak", fo.DEFAULT_LEARN_STREAK))
            max_att = fo.DEFAULT_LEARN_MAX_ATTEMPTS
            tol = cfg["tol"]
            n_targets = len(targets)
            plan_txt = ", ".join(f"{t:g}" for t in targets)
            self._event(
                "FO", None, b"",
                f"INLEREN start: {plan_txt}  (tot {streak}× OK, max {max_att}/gewicht, "
                f"gpr={gpr_start:.5f})",
            )
            for ti, target in enumerate(targets):
                self._fo_check_cancel()
                errors = []
                attempts = 0
                converged = False
                while attempts < max_att:
                    self._fo_check_cancel()
                    attempts += 1
                    base = 100 * ti / n_targets
                    span = 100 / n_targets
                    pct = base + span * min(attempts, max_att) / max_att
                    cur = fo.current_ok_streak(errors, tol)
                    self._ui(lambda ti=ti, n=n_targets, target=target, a=attempts,
                             s=streak, cur=cur, pct=pct: (
                        self.fo_progress.configure(value=pct),
                        self.fo_status_lbl.config(
                            text=(
                                f"Inleren {target:g} g  "
                                f"(gewicht {ti + 1}/{n} · poging {a} · OK-reeks {cur}/{s})…"
                            ),
                        ),
                    ))
                    rec = self._fo_one_dose(
                        cfg, target, "inleren",
                        progress_lo=pct, progress_hi=min(99, pct + span * 0.5 / max_att),
                    )
                    learn_history.append(rec)
                    self._fo_save_cal_with_history(cfg)
                    ep = rec.get("error_pct")
                    errors.append(ep)
                    dispensed = rec.get("dispensed_g")
                    if dispensed is not None and ep is not None:
                        flag = "OK" if abs(ep) <= tol else "CHECK"
                        self._event(
                            "FO", None, b"",
                            f"Inleer {target:g}g #{attempts}: {dispensed:.4f} g "
                            f"({ep:+.2f}%) [{flag}]  gpr={cfg['gpr']:.5f}",
                        )
                    if fo.streak_ok(errors, streak, tol):
                        converged = True
                        self._event(
                            "FO", None, b"",
                            f"→ {target:g} g stabiel ({streak}× binnen {tol:g}%) na {attempts} doses",
                        )
                        break
                    if pause > 0:
                        self._fo_check_cancel()
                        time.sleep(pause)

                target_results.append({
                    "target_g": target,
                    "attempts": attempts,
                    "converged": converged,
                    "errors": errors,
                })
                if not converged:
                    raise RuntimeError(
                        f"{target:g} g niet stabiel na {max_att} pogingen "
                        f"(nodig: {streak}× achter elkaar binnen {tol:g}%)"
                    )
                if ti + 1 < n_targets and pause > 0:
                    self._fo_check_cancel()
                    time.sleep(pause)

            # Keep the gpr that actually worked during learn (anchor). Finalize is
            # diagnostic only — applying it previously pulled 1.332 → 1.274 and
            # broke the next session.
            gpr_working = float(gpr_start)
            gpr_diag = fo.finalize_gpr_from_history(learn_history, gpr_working)
            cfg["gpr"] = gpr_working
            report = fo.learn_report(
                learn_history, gpr_start, gpr_working, tol,
                streak=streak, target_results=target_results,
            )
            report["gpr_diag"] = gpr_diag
            stamp = datetime.now().strftime("%Y%m%d_%H%M%S")
            payload = {
                "mode": "forward_only_learn",
                "node": cfg["node"], "motor": cfg["motor"], "fluid": cfg["fluid"],
                "plan": plan_txt,
                "streak": streak,
                "max_attempts": max_att,
                "pump_steps_per_rev": cfg["steps_rev"],
                "g_per_pump_rev_start": gpr_start,
                "g_per_pump_rev_working": gpr_working,
                "g_per_pump_rev": gpr_working,
                "g_per_pump_rev_diag": gpr_diag,
                "base_yield_ema": self.fo_base_yield,
                "tolerance_pct": tol,
                "learn_pause_s": pause,
                "report": report,
                "target_results": target_results,
                "history": learn_history,
                "started": stamp,
            }
            path = self._fo_save_learn_profile(cfg, payload)
            self.fo_cal = payload
            self.fo_cal_path = path

            def _done():
                self.fo_gpr.set(f"{gpr_working:.5f}")
                parts = [f"{t['target_g']:g}g:{t['attempts']}×" for t in target_results]
                color = "#26a269" if report["pass"] else "#c88800"
                self.fo_status_lbl.config(
                    text=(
                        f"Inleren opgeslagen: g/omw={gpr_working:.4f}  "
                        f"yield={self.fo_base_yield:.3f}  ({' '.join(parts)})  → {path.name}"
                    ),
                    foreground=color,
                )
                self.fo_cal_lbl.config(
                    text=(
                        f"Profiel (inleren): {path.name}  →  g/omw={gpr_working:.4f}  "
                        f"yield={self.fo_base_yield:.3f}"
                    ),
                    foreground=color,
                )
                self._fo_update_hist_lbl()

            self._ui(_done)
            self._event(
                "FO", None, b"",
                f"INLEREN opgeslagen gpr={gpr_working:.5f} yield={self.fo_base_yield:.3f}  "
                f"{report['ok']}/{report['n']} OK  → {path.name} + fo_profile_"
                f"{cfg['node']}_m{cfg['motor']}.json",
            )
        except Cancelled:
            error = "geannuleerd"
        except Exception as exc:
            error = str(exc)
            self._event("ERR", None, b"", f"FO inleren: {exc}")
        self._ui(lambda: self._fo_finished(error, "inleren"))

    def _fo_dose_worker(self, cfg):
        error = ""
        try:
            self._fo_ensure_ready(cfg)
            rec = self._fo_one_dose(cfg, cfg["target"], "productie")
            self._fo_save_cal_with_history(cfg)
            dispensed = rec.get("dispensed_g")
            err_pct = rec.get("error_pct")
            if dispensed is None:
                msg = f"Gedoseerd open-loop: {rec['steps']} stappen (geen schaalcheck)."
            else:
                flag = "OK" if fo.within_tolerance(dispensed, cfg["target"], cfg["tol"]) else "CHECK"
                msg = f"productie: {dispensed:.4f} g ({err_pct:+.2f}%) [{flag}]"
            self._event("FO", None, b"", msg)
            self._ui(lambda m=msg: self.fo_status_lbl.config(text=m, foreground="green"))
        except Cancelled:
            error = "geannuleerd"
        except Exception as exc:
            error = str(exc)
            self._event("ERR", None, b"", f"FO dose: {exc}")
        self._ui(lambda: self._fo_finished(error, "dose"))

    def _fo_save_cal_with_history(self, cfg):
        FO_DIR.mkdir(parents=True, exist_ok=True)
        stamp = datetime.now().strftime("%Y%m%d_%H%M%S")
        payload = {
            "mode": "forward_only_live",
            "node": cfg["node"], "motor": cfg["motor"], "fluid": cfg["fluid"],
            "pump_steps_per_rev": cfg["steps_rev"],
            "g_per_pump_rev": float(self.fo_gpr.get().replace(",", ".")),
            "base_yield_ema": self.fo_base_yield,
            "history": self.fo_history[-50:],
            "updated": stamp,
        }
        if self.fo_cal:
            payload["calibration"] = {
                k: self.fo_cal.get(k) for k in ("g_per_pump_rev", "anchor_sd_g", "anchor_cv_pct", "started")
            }
        path = FO_DIR / f"fo_live_{cfg['node']}_m{cfg['motor']}.json"
        path.write_text(json.dumps(payload, indent=2), encoding="utf-8")

    def _fo_row(self, rec):
        idx = rec.get("index", "")
        kind = rec.get("kind", "")
        target = "" if rec.get("target_g") is None else f"{rec['target_g']:g}"
        disp = "" if rec.get("dispensed_g") is None else f"{rec['dispensed_g']:.4f}"
        pct = "" if rec.get("error_pct") is None else f"{rec['error_pct']:+.2f}"
        steps = rec.get("steps", "")
        gpr = "" if rec.get("gpr") is None else f"{rec['gpr']:.4f}"
        dur = rec.get("duration_s", "")
        try:
            tol = float(self.fo_tol.get() or 1)
        except ValueError:
            tol = 1.0
        bad = rec.get("error_pct") is not None and abs(rec["error_pct"]) > tol
        tag = "bad" if bad else "ok"
        self.fo_tree.insert("", "end", values=(idx, kind, target, disp, pct, steps, gpr, dur), tags=(tag,))

    def _fo_finished(self, error, kind=""):
        self.busy = None
        self._fo_set_busy(False)
        self.fo_progress["value"] = 0 if error else 100
        self._fo_update_primed_lbl()
        if error:
            self.fo_status_lbl.config(text=f"Gestopt: {error}", foreground="#c01c28")
        elif kind in ("kalibratie", "inleren"):
            pass  # status set by worker
        elif kind != "dose":
            self.fo_status_lbl.config(text=f"Klaar ({kind})", foreground="green")

    def cancel_forward_only(self):
        if not self.busy or not str(self.busy).startswith("forward-only"):
            return
        if self.fo_cancel.is_set():
            return
        self.fo_cancel.set()
        self._event("INFO", None, b"", ">> FORWARD-ONLY stoppen")
        try:
            node, motor = int(self.node.get()), int(self.motor.get())
            decel = int(self.decel.get())
        except ValueError:
            return
        threading.Thread(
            target=lambda: self._sequence(node, motor, COMMANDS["STOP"], 0, 0, 0, decel),
            daemon=True,
        ).start()

    def open_report(self):
        if self.last_report and self.last_report.exists():
            self._open_path(self.last_report)
        else:
            self._open_dir(DOSE_DIR)

    # ---------- test plan editor ----------

    def _plan_refresh(self, select=None):
        self.plan_tree.delete(*self.plan_tree.get_children())
        for i, (target, repeats) in enumerate(self.plan):
            self.plan_tree.insert("", "end", iid=str(i), values=(f"{target:g}", repeats))
        if select is not None and 0 <= select < len(self.plan):
            self.plan_tree.selection_set(str(select))
        count, grams = sum(r for _, r in self.plan), sum(t * r for t, r in self.plan)
        self.plan_total_lbl.config(text=f"{count} doseringen, totaal {grams:g} g")
        self.plan_text.set(dosing.plan_to_text(self.plan))

    def _plan_index(self):
        selection = self.plan_tree.selection()
        return int(selection[0]) if selection else None

    def _plan_selected(self):
        index = self._plan_index()
        if index is not None:
            target, repeats = self.plan[index]
            self.plan_target.set(f"{target:g}")
            self.plan_reps.set(str(repeats))

    def _plan_entry(self):
        try:
            item = (float(self.plan_target.get().replace(",", ".")), int(self.plan_reps.get()))
        except ValueError:
            messagebox.showerror("Ongeldige invoer", "Doel is een getal (g), aantal een geheel getal.")
            return None
        if item[0] <= 0 or item[1] < 1:
            messagebox.showerror("Ongeldige invoer", "Doel > 0 g en aantal >= 1.")
            return None
        return item

    def plan_add(self):
        item = self._plan_entry()
        if item:
            self.plan.append(item)
            self._plan_refresh(len(self.plan) - 1)

    def plan_update(self):
        index, item = self._plan_index(), self._plan_entry()
        if index is None:
            messagebox.showinfo("Testplan", "Selecteer eerst een regel.")
        elif item:
            self.plan[index] = item
            self._plan_refresh(index)

    def plan_remove(self):
        index = self._plan_index()
        if index is not None:
            del self.plan[index]
            self._plan_refresh(min(index, len(self.plan) - 1))

    def plan_move(self, delta: int):
        index = self._plan_index()
        if index is not None and 0 <= index + delta < len(self.plan):
            self.plan[index], self.plan[index + delta] = self.plan[index + delta], self.plan[index]
            self._plan_refresh(index + delta)

    def plan_apply_text(self):
        try:
            plan = dosing.parse_plan_text(self.plan_text.get())
        except ValueError as exc:
            messagebox.showerror("Testplan", str(exc))
            return
        if not plan:
            messagebox.showerror("Testplan", "Leeg plan. Voorbeeld: 10x10, 10x20, 5x50")
            return
        self.plan = plan
        self._plan_refresh()

    def plan_save(self):
        PLAN_DIR.mkdir(parents=True, exist_ok=True)
        path = filedialog.asksaveasfilename(initialdir=PLAN_DIR, defaultextension=".json",
                                            initialfile="doseerplan.json", filetypes=[("JSON", "*.json")])
        if not path:
            return
        data = {"plan": [{"target_g": t, "repeats": r} for t, r in self.plan],
                "settings": {key: var.get() for key, var in self._dose_setting_vars().items()}}
        try:
            Path(path).write_text(json.dumps(data, indent=2), encoding="utf-8")
        except OSError as exc:
            messagebox.showerror("Plan opslaan", str(exc))

    def plan_load(self):
        PLAN_DIR.mkdir(parents=True, exist_ok=True)
        path = filedialog.askopenfilename(initialdir=PLAN_DIR, filetypes=[("JSON", "*.json")])
        if not path:
            return
        try:
            data = json.loads(Path(path).read_text(encoding="utf-8"))
            plan = [(float(s["target_g"]), int(s["repeats"])) for s in data["plan"]]
            if not plan or any(t <= 0 or r < 1 for t, r in plan):
                raise ValueError("leeg plan of ongeldige regel")
        except (OSError, ValueError, KeyError, TypeError) as exc:
            messagebox.showerror("Plan laden", f"{path}\n\n{exc}")
            return
        for key, var in self._dose_setting_vars().items():
            if key in data.get("settings", {}):
                var.set(data["settings"][key])
        self.plan = plan
        self._plan_refresh()

    # ---------- log ----------

    def _event(self, direction, can_id, data: bytes, info: str = ""):
        """Thread-safe: queue an event for the UI thread."""
        self.events.put((datetime.now(), direction, can_id, data, info))

    def _ui(self, fn):
        """Thread-safe: run fn on the Tk thread (Tk widgets must not be touched from workers)."""
        self.ui_calls.put(fn)

    def _drain_events(self):
        while True:
            try:
                fn = self.ui_calls.get_nowait()
            except queue.Empty:
                break
            fn()
        batch = 0
        while batch < 500:
            try:
                ts, direction, can_id, data, info = self.events.get_nowait()
            except queue.Empty:
                break
            batch += 1
            is_hb = (direction == "RX" and len(data) >= 4 and data[:4] == b"\xff\xff\x00\xaa")
            if direction in self.stats:
                self.stats[direction] += 1
            if is_hb:
                self.stats["HB"] += 1
                self.last_heartbeat[can_id] = ts
            if not info and can_id is not None:
                info = decode(can_id, data, direction)
            id_text = "" if can_id is None else f"{can_id} (0x{can_id:03X})"
            data_text = " ".join(f"{b:02X}" for b in data)
            row = (ts.strftime("%H:%M:%S.%f")[:-3], direction, id_text, data_text, info)
            self.rows.append(row)
            if self.csv_writer:
                self.csv_writer.writerow([ts.isoformat(), direction, "" if can_id is None else can_id,
                                          "" if can_id is None else f"0x{can_id:03X}", data_text, info])
            if self.paused.get() or (is_hb and not self.show_hb.get()) or \
                    (direction == "TX" and not self.show_tx.get()):
                continue
            self.tree.insert("", "end", values=row, tags=("HB" if is_hb else direction,))
        if batch:
            if self.csv_file:
                self.csv_file.flush()
            children = self.tree.get_children()
            if len(children) > 5000:
                self.tree.delete(*children[:len(children) - 5000])
            if self.autoscroll.get() and children:
                self.tree.see(self.tree.get_children()[-1])
        self.root.after(50, self._drain_events)

    def _refresh_stats(self):
        s = self.stats
        self.stats_lbl.config(text=f"TX {s['TX']}   RX {s['RX']}   ERR {s['ERR']}   HB {s['HB']}")
        now = datetime.now()
        hb = ", ".join(f"node {n}: {(now - t).total_seconds():.0f}s geleden"
                       for n, t in sorted(self.last_heartbeat.items()))
        self.hb_lbl.config(text=f"Laatste heartbeat - {hb}" if hb else "Nog geen heartbeat ontvangen")
        try:
            node, motor = int(self.node.get()), int(self.motor.get())
            pos = self.commanded.get(motor, 0)
            reported = self.reported.get((node, motor))
            fb = "geen terugmelding" if reported is None else f"gemeld {reported} ({reported / STEPS_PER_REV:.3f} omw)"
            self.cmd_pos_lbl.config(text=f"Motor {motor}: gecommandeerd {pos} "
                                         f"({pos / STEPS_PER_REV:.3f} omw), {fb}")
        except ValueError:
            pass
        self._refresh_weight()
        self.root.after(500, self._refresh_stats)

    def _refresh_weight(self):
        reading = self.scale_reading
        if not self.scale or not reading:
            self.weight_lbl.config(text="---.---- g", fg="gray")
            return
        age = time.monotonic() - self.scale_reading_at
        stale = age > 3 * SCALE_POLL_S + 1.0
        state = f"oud {age:.0f}s" if stale else ("stabiel" if reading.stable else "onstabiel")
        if not stale and not self.scale_status_known:
            state += " (geen S/D-status!)"
        color = "gray" if stale else ("#26a269" if reading.stable else "#c88800")
        self.weight_lbl.config(text=f"{reading.weight:9.4f} {reading.unit}  {state}", fg=color)

    def add_note(self):
        win = tk.Toplevel(self.root)
        win.title("Notitie in log")
        var = tk.StringVar()
        entry = ttk.Entry(win, textvariable=var, width=60)
        entry.pack(padx=8, pady=8)
        entry.focus()

        def ok(_=None):
            if var.get().strip():
                self._event("INFO", None, b"", f"NOTITIE: {var.get().strip()}")
            win.destroy()

        entry.bind("<Return>", ok)
        ttk.Button(win, text="OK", command=ok).pack(pady=(0, 8))

    def clear_log(self):
        self.tree.delete(*self.tree.get_children())
        self.rows.clear()
        self.stats = {"TX": 0, "RX": 0, "ERR": 0, "HB": 0}

    def save_csv(self):
        path = filedialog.asksaveasfilename(defaultextension=".csv", filetypes=[("CSV", "*.csv")],
                                            initialfile=f"pump_debug_{datetime.now():%Y%m%d_%H%M%S}.csv")
        if not path:
            return
        with open(path, "w", newline="", encoding="utf-8") as f:
            writer = csv.writer(f)
            writer.writerow(["time", "dir", "id", "data", "decode"])
            writer.writerows(self.rows)

    def on_close(self):
        if self.busy and not messagebox.askyesno("Afsluiten", f"Er loopt nog een {self.busy}. Stoppen en afsluiten?"):
            return
        self.cancel_calibration()
        self.cancel_dose()
        self.cancel_forward_only()
        if self.scale:
            self.disconnect_scale()
        if self.bus:
            self.disconnect()
        self.root.destroy()


def main():
    root = tk.Tk()
    try:
        ttk.Style().theme_use("vista")
    except tk.TclError:
        pass
    PumpDebugApp(root)
    root.mainloop()


if __name__ == "__main__":
    main()
