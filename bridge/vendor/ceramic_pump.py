"""Ceramic pump operations, using the repository's proven board encoder.

No GUI, backend, calibration database or adaptive dosing dependency.
Software stop is best effort; loss of CAN cannot physically stop a drive.
"""
import csv
import io
import os
from collections import deque
import json
import math
import re
import statistics
import threading
import time
from datetime import datetime
from pathlib import Path

import can
from vwa_stepper_controller import VWAStepperController, VWACommand as Command

MOTOR_STEPS_PER_REV = 25600


class Cancelled(RuntimeError):
    pass


def positive(value, name):
    value = float(str(value).replace(",", "."))
    if not math.isfinite(value) or value <= 0:
        raise ValueError(f"{name} moet eindig en > 0 zijn")
    return value


def integer(value, name, maximum=2147483647):
    number = positive(value, name)
    if not number.is_integer() or number > maximum:
        raise ValueError(f"{name} moet een geheel getal tussen 1 en {maximum} zijn")
    return int(number)


def move_timeout(distance, speed, accel, decel, margin=3.0):
    return abs(distance) / max(speed, 1) + speed / max(accel, 1) + speed / max(decel, 1) + margin


def safe_name(text):
    return re.sub(r"[^a-z0-9_-]+", "_", text.strip().lower()).strip("_") or "fluid"


def parse_position_feedback(data):
    if len(data) != 8 or data[1] != 1 or data[3] != 0xAA or data[0] not in (1, 74, 75, 76):
        return None
    return data[2], int.from_bytes(data[4:], "little", signed=True), data[0] in (74, 75)


def mtsics_stable(raw):
    matches = re.findall(r"^\s*S(?:I)?\s+([SD])\s+[-+]?\s*\d", raw or "", re.MULTILINE)
    return matches[-1] == "S" if matches else None


def reading_grams(reading, requested_at):
    """Reject the shared communicator's stale fallback and unknown stability."""
    if reading is None or reading.timestamp < requested_at:
        raise ValueError("Geen verse weegschaalmeting")
    matches = re.findall(r"^\s*S(?:I)?\s+([SD])\s+([+-]?\s*\d+(?:\.\d*)?)\s+([a-zA-Z]+)\s*$",
                         reading.raw_data or "", re.MULTILINE)
    if not matches:
        raise ValueError("Geen expliciete MT-SICS S/D-status")
    status, value, unit = matches[-1]
    factor = {"g": 1.0, "mg": 0.001, "kg": 1000.0}.get(unit.lower())
    if factor is None or not math.isfinite(reading.weight):
        raise ValueError("Onbekende gewichtseenheid of ongeldig gewicht")
    # Use weight and status from the SAME complete line. The shared parser may
    # choose the first line when serial/TCP returns multiple samples at once.
    weight = float(value.replace(" ", "")) * factor
    if not math.isfinite(weight):
        raise ValueError("Ongeldig gewicht")
    return weight, status == "S"


def fresh_scale_read(scale, raw_sink=None):
    """Call under the scale lock; discard replies buffered BEFORE this request."""
    connection = getattr(scale, "serial_connection", None)
    if connection is not None:
        connection.reset_input_buffer()
    requested = time.time()
    reading = scale.get_reading()
    if raw_sink is not None:
        raw_sink({"requested_at": requested, "received_at": getattr(reading, "timestamp", None),
                  "raw_data": getattr(reading, "raw_data", None)})
    return reading_grams(reading, requested)


class PumpConfig:
    def __init__(self, node=610, response_node=None, motor=2, steps_rev=25600,
                 speed=51200, accel=51200, decel=51200, fluid="water", density=None, pump_type="INV-13", pump_id="unknown", stroke_setting="unknown", temperature_c=None, density_source="unknown",
                 scale_id="unknown", scale_resolution_g=None, tubing="unknown", priming_confirmed=False):
        if pump_type not in ("INV-13", "INV-36"):
            raise ValueError("Pomptype moet INV-13 of INV-36 zijn")
        self.pump_type = pump_type
        self.pump_id = str(pump_id).strip() or "unknown"
        self.stroke_setting = str(stroke_setting).strip() or "unknown"
        self.temperature_c = None if temperature_c in (None, "") else float(str(temperature_c).replace(",", "."))
        if self.temperature_c is not None and not math.isfinite(self.temperature_c):
            raise ValueError("Temperatuur moet eindig zijn")
        self.density_source = str(density_source).strip() or "unknown"
        self.scale_id = str(scale_id).strip() or "unknown"
        self.scale_resolution_g = None if scale_resolution_g in (None, "") else positive(scale_resolution_g, "Schaalresolutie")
        self.tubing = str(tubing).strip() or "unknown"
        self.priming_confirmed = bool(priming_confirmed)
        self.node = integer(node, "CAN-node", 2047)
        self.response_node = integer(response_node if response_node is not None else node,
                                     "Antwoordnode", 2047)
        self.motor = integer(motor, "Motor", 8)
        self.steps_rev = integer(steps_rev, "Stappen per pompomwenteling")
        self.motor_steps_rev = MOTOR_STEPS_PER_REV
        self.protocol_position_split_steps = 51200  # Existing repository encoder; independent of physical pump revolution.
        self.speed = integer(speed, "Snelheid")
        self.accel = integer(accel, "Acceleratie")
        self.decel = integer(decel, "Deceleratie")
        self.fluid = str(fluid).strip()
        if not self.fluid:
            raise ValueError("Vloeistofnaam ontbreekt")
        self.density = None if density in (None, "") else positive(density, "Dichtheid")

    def context(self):
        return {key: getattr(self, key) for key in
                ("pump_type", "pump_id", "stroke_setting", "temperature_c", "density_source", "scale_id", "scale_resolution_g", "tubing", "node", "response_node", "motor", "steps_rev", "motor_steps_rev", "protocol_position_split_steps", "speed", "accel", "decel", "fluid", "density")}

    def steps(self, revolutions):
        result = round(positive(revolutions, "Omwentelingen") * self.steps_rev)
        return integer(result, "Stappen")


class SessionLog:
    def __init__(self, directory, sink=lambda text: None):
        directory = Path(directory)
        directory.mkdir(parents=True, exist_ok=True)
        self.path = directory / f"ceramic_{datetime.now():%Y%m%d_%H%M%S_%f}.csv"
        self.file = self.path.open("w", encoding="utf-8", newline="")
        self.writer = csv.writer(self.file)
        self.writer.writerow(["time", "kind", "details"])
        self.lock = threading.Lock()
        self.sink = sink

    def __call__(self, kind, details):
        text = json.dumps(details, ensure_ascii=False) if isinstance(details, dict) else str(details)
        with self.lock:
            self.writer.writerow([datetime.now().astimezone().isoformat(), kind, text])
            self.file.flush()
        self.sink(f"{kind}: {text}")

    def close(self):
        with self.lock:
            self.file.close()


class Pump:
    def __init__(self, bus, config, log=lambda kind, details: None):
        self.bus, self.cfg, self.log = bus, config, log
        self.cancel = threading.Event()
        self.closed = threading.Event()
        self.tx_lock = threading.Lock()
        self.sequence_lock = threading.Lock()
        self.condition = threading.Condition()
        self.feedback = {}
        self.local = threading.local()
        self.fault = None
        self.error_registers = {}
        self.initializing = False
        self.cancel_generation = 0
        self.ready = self.primed = False
        self.position = None
        self.controller = VWAStepperController(self)
        self.controller.set_destination_node(config.node)
        self.rx_thread = threading.Thread(target=self._receive, daemon=True)
        self.rx_thread.start()

    def check(self):
        if self.cancel.is_set():
            raise Cancelled("Gestopt; opnieuw initialiseren vereist")
        if self.fault:
            raise RuntimeError(self.fault)
        if self.closed.is_set():
            raise RuntimeError("CAN-verbinding gesloten")
        if not self.initializing and any(self.error_registers.values()):
            raise RuntimeError(f"Motorfoutregisters: {self.error_registers}")

    def sleep(self, seconds):
        if self.cancel.wait(seconds):
            self.check()
        self.check()

    def send_message(self, message):
        """Adapter for the existing encoder, cancellable before EACH frame."""
        with self.tx_lock:
            if not getattr(self.local, "emergency", False):
                self.check()
            self.bus.send(can.Message(arbitration_id=message.message_id,
                                      data=message.data, is_extended_id=False), timeout=0.2)
            self.log("TX", {"node": message.message_id, "data": message.data.hex(" ")})
        return True

    def _receive(self):
        try:
            while not self.closed.is_set():
                message = self.bus.recv(timeout=0.1)
                if message is None:
                    continue
                if message.is_error_frame:
                    raise RuntimeError("CAN error frame")
                data = bytes(message.data)
                self.log("RX", {"node": message.arbitration_id, "data": data.hex(" ")})
                if (message.is_extended_id or message.is_remote_frame or
                        message.arbitration_id != self.cfg.response_node or len(data) != 8 or
                        data[2] != self.cfg.motor or data[3] != 0xAA):
                    continue
                # CAN_Task.cpp postErrors: records 0x81..0x88 form an error
                # snapshot. 0xFF marks its last part; value zero CLEARS errors.
                if 0x81 <= data[1] <= 0x88:
                    register = data[1] - 0x80
                    value = int.from_bytes(data[4:], "little", signed=False)
                    self.error_registers[register] = value
                    self.log("MOTOR_ERROR_STATUS", {"register": register, "value": value})
                    if value and not self.initializing:
                        self.ready = self.primed = False
                    continue
                if data[0] == 0xFF and data[1] == 1:
                    raise RuntimeError(f"Onbekende motorfout: {data.hex(' ')}")
                with self.condition:
                    self.feedback[(data[0], data[1])] = (
                        time.perf_counter(), int.from_bytes(data[4:], "little", signed=True))
                    self.condition.notify_all()
        except Exception as exc:
            self.fault = f"CAN-ontvangst mislukt: {exc}"
            self.ready = self.primed = False
            self.log("ERROR", self.fault)
            # Keep the receiver alive only on normal operation; no new moves on a fault.
            try:
                self.abort(wait_ack=False)
            except Exception:
                pass  # The failure is already recorded; receiver exits on a CAN fault.

    def command(self, code, position=0, speed=0, accel=0, decel=0):
        with self.sequence_lock:
            self.check()
            if not self.controller._send_command(self.cfg.motor, code, position, speed, accel, decel):
                self.check()
                raise RuntimeError(f"CAN-command {code.name} mislukt")

    def wait_response(self, code, since, timeout, expected=None, emergency=False):
        deadline = time.perf_counter() + timeout
        with self.condition:
            while True:
                if not emergency:
                    self.check()
                response = self.feedback.get((int(code), 1))
                if response and response[0] >= since:
                    if expected is not None and response[1] != expected:
                        raise RuntimeError(f"Positie {response[1]} != doel {expected}")
                    return response[1]
                if time.perf_counter() >= deadline:
                    raise TimeoutError(f"Geen bevestiging voor {code.name}")
                self.condition.wait(0.05)

    def cancel_now(self):
        with self.tx_lock:
            self.cancel_generation += 1
            self.cancel.set()
        self.ready = self.primed = False

    def abort(self, stop=False, wait_ack=True, user_requested=True):
        # Set the cancellation flag while holding the same lock as frame transmission.
        # Therefore an interrupted sequence can never execute MOVE after STOP/ABORT.
        with self.tx_lock:
            if user_requested:
                self.cancel_generation += 1
            self.cancel.set()
        self.ready = self.primed = False
        self.position = None
        # Firmware PLC_STOP only posts a response; PLC_ABORT transitions into
        # homing (stAbortBusy -> stPreHomeStart) and never reaches ABORTED.
        # PLC_INIT disables the drive and resets/restores it; normal mode then
        # reports INIT_DONE without issuing a move or home command.
        code = Command.PLC_INIT
        with self.sequence_lock:
            self.local.emergency = True
            self.initializing = True
            try:
                since = time.perf_counter()
                # INIT ignores parameters. Send its execute frame immediately
                # through the shared encoder rather than waiting seven frames.
                if not self.controller._send_vwa_can_message(1, self.cfg.motor, int(code), 0, add_delay=False):
                    raise RuntimeError("Stopopdracht kon niet worden verzonden")
                if wait_ack:
                    self.wait_response(code, since, 10, emergency=True)
                self.log("STOP", {"button": "STOP" if stop else "ABORT", "command": code.name, "confirmed": wait_ack})
            except Exception as exc:
                self.log("ERROR", f"Stop niet bevestigd: {exc}. Controleer fysieke noodstop.")
                raise
            finally:
                self.local.emergency = False
                self.initializing = False

    def initialize(self):
        """Preserve original _init_start_seq: INIT, 1 s, START, 0.5 s.

        This is the existing operational sequence, not an INIT_DONE handshake.
        The software command counter starts at zero as in the original GUI;
        no ZERO command or physical-position confirmation is added here.
        """
        generation = self.cancel_generation
        if self.fault:
            raise RuntimeError("CAN-fout: verbinding verbreken en opnieuw verbinden")
        with self.tx_lock:
            if self.cancel_generation != generation:
                raise Cancelled("Initialiseren geannuleerd")
            self.cancel.clear()
        self.ready = self.primed = False
        self.position = None
        self.initializing = True
        try:
            self.command(Command.PLC_INIT, speed=self.cfg.speed, accel=self.cfg.accel, decel=self.cfg.decel)
            self.sleep(1.0)
            self.command(Command.PLC_START, speed=self.cfg.speed, accel=self.cfg.accel, decel=self.cfg.decel)
            self.sleep(0.5)
        finally:
            self.initializing = False
        self.check()
        self.position = 0
        self.ready = True
        self.log("READY", {**self.cfg.context(), "initialization_sequence": "INIT; wait 1 s; START; wait 0.5 s",
                           "confirmation": "commands_sent; no INIT_DONE required",
                           "position_basis": "software_command_counter; physical origin unconfirmed"})

    def move(self, steps):
        self.check()
        steps = integer(steps, "Stappen")
        if not self.ready or self.position is None:
            raise RuntimeError("Eerst initialiseren")
        target = self.position + steps
        if target > 2147483647:
            raise ValueError("Positieteller vol; opnieuw initialiseren en primen")
        since = time.perf_counter()
        try:
            self.command(Command.PLC_MOVE_TO, target, self.cfg.speed, self.cfg.accel, self.cfg.decel)
            self.wait_response(Command.PLC_MOVE_TO, since,
                               move_timeout(steps, self.cfg.speed, self.cfg.accel, self.cfg.decel, 5), target)
            self.check()
        except Exception:
            # A failed/partial move makes the absolute counter uncertain. Never
            # permit another MOVE until a confirmed initialization resets it.
            if not self.cancel.is_set():
                try:
                    self.abort()
                except Exception:
                    pass  # Keep the original motion failure; abort failure is logged.
            raise
        self.position = target
        self.log("MOVE", {"steps": steps, "rev": steps / self.cfg.steps_rev, "position": target})

    def prime(self, revolutions):
        count = integer(revolutions, "Prime-omwentelingen", 10000)
        self.move(self.cfg.steps(count))
        self.primed = True
        self.log("PRIME", {"rev": count})

    def close(self):
        self.closed.set()
        self.rx_thread.join(timeout=1)
        self.bus.shutdown()


class MeasurementPolicy:
    """Operator-selected measurement limits; never manufacturer specifications."""
    def __init__(self, window_s=1.2, range_g=0.002, timeout_s=30,
                 settle_min_s=2, sample_interval_s=0.2, min_samples=3):
        self.window_s = positive(window_s, "Stabiliteitsvenster")
        self.range_g = positive(range_g, "Maximale massavariatie")
        self.timeout_s = positive(timeout_s, "Weegtimeout")
        self.settle_min_s = positive(settle_min_s, "Minimale uitlooptijd")
        self.sample_interval_s = positive(sample_interval_s, "Meetinterval")
        self.min_samples = integer(min_samples, "Minimum samples", 100)
        if self.min_samples < 2 or self.timeout_s <= max(self.window_s, self.settle_min_s):
            raise ValueError("Timeout moet groter zijn dan venster en uitlooptijd; minimaal 2 samples")

    def context(self):
        return dict(vars(self))


def bounded_phase(value):
    value = float(str(value).replace(",", "."))
    if not math.isfinite(value) or not 0 <= value < 1:
        raise ValueError("Startfase moet tussen 0 (inclusief) en 1 (exclusief) liggen")
    return value


def atomic_text(path, content):
    """Same-volume replace: a failed export cannot truncate its previous version."""
    path = Path(path)
    temporary = path.with_name(path.name + ".tmp")
    with temporary.open("w", encoding="utf-8", newline="") as stream:
        stream.write(content)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)


class Measurements:
    def __init__(self, pump, scale, scale_lock, log, directory, weight_sink=lambda text: None,
                 policy=None):
        self.pump, self.scale, self.scale_lock = pump, scale, scale_lock
        self.log, self.directory, self.weight_sink = log, Path(directory), weight_sink
        self.policy = policy or MeasurementPolicy()
        self.records, self.scale_samples = [], []
        self.result_sink = lambda row: None
        self.progress_sink = lambda text: None
        self.session_id = datetime.now().strftime("%Y%m%d_%H%M%S_%f")
        self.path = self.directory / f"results_{self.session_id}.json"
        self.journal_path = self.path.with_suffix(".jsonl")
        self.active_id, self.stage = None, "idle"
        self.last_window = None
        self.started = time.perf_counter()

    def journal(self, event, payload):
        self.directory.mkdir(parents=True, exist_ok=True)
        row = {"event": event, "time": datetime.now().astimezone().isoformat(),
               "session_id": self.session_id, "payload": payload}
        with self.journal_path.open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(row, ensure_ascii=False, allow_nan=False) + "\n")
            stream.flush()
            os.fsync(stream.fileno())

    def sample(self):
        self.pump.check()
        sample = {"measurement_id": self.active_id, "stage": self.stage,
                  "elapsed_session_s": time.perf_counter() - self.started,
                  "time": datetime.now().astimezone().isoformat()}
        raw = []
        try:
            with self.scale_lock:
                weight, stable = fresh_scale_read(self.scale, raw.append)
            sample.update(raw[-1] if raw else {})
            sample.update(g=weight, stable=stable, status="valid")
        except ValueError as exc:
            sample.update(raw[-1] if raw else {})
            sample.update(status="invalid", error=str(exc))
            weight, stable = None, False
        sample["request_elapsed_s"] = sample["elapsed_session_s"]
        sample["elapsed_session_s"] = time.perf_counter() - self.started
        self.scale_samples.append(sample)
        self.journal("scale_sample", sample)
        if weight is not None:
            self.weight_sink(f"{weight:.5f} g {'stabiel' if stable else 'dynamisch'}")
        return weight, stable

    def stable_weight(self, hold=None, timeout=None, minimum_s=0):
        """Require consecutive S replies AND bounded mass range over a time window."""
        window_s = self.policy.window_s if hold is None else max(float(hold), 0)
        timeout = self.policy.timeout_s if timeout is None else positive(timeout, "Timeout")
        start = time.perf_counter()
        window = deque()
        self.last_window = None
        while time.perf_counter() - start < timeout:
            weight, stable = self.sample()
            now = time.perf_counter()
            if weight is None or not stable:
                window.clear()
            else:
                window.append((now, weight))
                # Keep one sample at/before the lower boundary to cover full duration.
                while len(window) > self.policy.min_samples and window[1][0] <= now - window_s:
                    window.popleft()
                values = [value for _, value in window]
                if (len(values) >= self.policy.min_samples and
                        now - window[0][0] >= window_s and now-start >= minimum_s and
                        max(values)-min(values) <= self.policy.range_g):
                    self.pump.check()
                    self.last_window = {"n": len(values), "range_g": max(values)-min(values),
                                        "window_s": now-window[0][0], "wait_s": now-start,
                                        "status": "stable_and_range_checked"}
                    return statistics.mean(values)
            self.pump.sleep(self.policy.sample_interval_s)
        raise TimeoutError(f"Geen stabiele massa binnen {timeout:g} s (S-status én variatie ≤ {self.policy.range_g:g} g)")

    def begin(self, kind, steps=0, target_g=None, metadata=None):
        record = {"measurement_id": f"{self.session_id}-{len(self.records)+1:04d}",
                  "kind": kind, "status": "running", "started": datetime.now().astimezone().isoformat(),
                  "primed": self.pump.primed, "priming_confirmed": self.pump.cfg.priming_confirmed,
                  "start_position_steps": self.pump.position,
                  "start_phase_rev": (self.pump.position % self.pump.cfg.steps_rev) / self.pump.cfg.steps_rev if self.pump.position is not None else None,
                  "rpm": self.pump.cfg.speed*60/self.pump.cfg.steps_rev,
                  "rpm_basis": "commanded speed; physical shaft speed not measured",
                  "steps": steps, "rev": steps/self.pump.cfg.steps_rev, "target_g": target_g,
                  "context": self.pump.cfg.context(), "stability_status": "unknown",
                  "policy": self.policy.context(), **(metadata or {})}
        self.records.append(record)
        self.active_id = record["measurement_id"]
        self.journal("measurement_started", record)
        self.save_results()
        return record

    def finish(self, record, started):
        record["elapsed_s"] = time.perf_counter()-started
        self.journal("measurement_finished", record)
        self.save_results()
        self.log(record["kind"], record)
        self.result_sink(dict(record))
        self.active_id, self.stage = None, "idle"

    def shot(self, steps, target_g=None, kind="CALIBRATION", metadata=None):
        if not self.pump.primed:
            raise RuntimeError("Eerst primen en uitloop boven opvang op weegschaal plaatsen")
        steps = integer(steps, "Stappen")
        if target_g is not None:
            target_g = positive(target_g, "Doelmassa")
        started = time.perf_counter()
        record = self.begin(kind, steps, target_g, metadata)
        try:
            self.stage = "baseline"
            self.progress_sink("Wachten op stabiele uitgangsmassa")
            record["before_g"] = self.stable_weight()
            record["before_window"] = self.last_window
            self.stage = "motion"
            self.progress_sink("Vooruit bewegen; wachten op CAN-bevestiging")
            moved_at = time.perf_counter()
            self.pump.move(steps)
            record["motion_s"] = time.perf_counter()-moved_at
            record["end_position_steps"] = self.pump.position
            self.stage = "settling"
            self.progress_sink("Uitloop meten; wachten op stabiele eindmassa")
            # Samples continue throughout settling; no fixed blind sleep.
            record["after_g"] = self.stable_weight(minimum_s=self.policy.settle_min_s)
            record["after_window"] = self.last_window
            record["settle_s"] = self.last_window["wait_s"] if self.last_window else None
            record["mass_g"] = record["after_g"]-record["before_g"]
            if record["mass_g"] < 0 or (record["mass_g"] == 0 and kind not in ("PROTOCOL", "REFERENCE")):
                raise RuntimeError("Geen positieve massa; controleer opvang, slang en weegschaal")
            record["g_per_rev"] = record["mass_g"]/record["rev"]
            record["yield_basis"] = "whole_revolutions" if steps % self.pump.cfg.steps_rev == 0 else "fractional_observation_not_linear_calibration"
            record["volume_ml"] = record["mass_g"]/self.pump.cfg.density if self.pump.cfg.density else None
            if target_g is not None:
                record["error_g"] = record["mass_g"]-target_g
                record["error_pct"] = 100*record["error_g"]/target_g
            record.update(status="completed", stability_status="stable_before_and_after")
            return record
        except Exception as exc:
            record.update(status="failed", error=str(exc))
            raise
        finally:
            self.finish(record, started)

    def blank(self, duration_s=5):
        """No motion: observe scale drift/dripping, without silently correcting doses."""
        duration_s = positive(duration_s, "Blancoduur")
        started = time.perf_counter()
        record = self.begin("BLANK", metadata={"requested_duration_s": duration_s})
        try:
            self.stage = "blank"
            self.progress_sink("Blancometing: drift/nadruppelen, geen motorbeweging")
            samples = []
            while time.perf_counter()-started < duration_s:
                weight, stable = self.sample()
                if weight is not None:
                    samples.append((time.perf_counter()-started, weight, stable))
                self.pump.sleep(self.policy.sample_interval_s)
            if len(samples) < 2:
                raise RuntimeError("Te weinig geldige samples voor blancometing")
            seconds = samples[-1][0]-samples[0][0]
            record.update(status="completed", before_g=samples[0][1], after_g=samples[-1][1],
                          mass_g=samples[-1][1]-samples[0][1], sample_count=len(samples),
                          range_g=max(r[1] for r in samples)-min(r[1] for r in samples),
                          drift_g_per_s=(samples[-1][1]-samples[0][1])/seconds if seconds else None,
                          all_scale_stable=all(r[2] for r in samples),
                          stability_status="observed_no_motion")
            return record
        except Exception as exc:
            record.update(status="failed", error=str(exc))
            raise
        finally:
            self.finish(record, started)

    def phase_shift(self, phase):
        phase = bounded_phase(phase)
        if not self.pump.primed or self.pump.position is None:
            raise RuntimeError("Faseverplaatsing vereist geinitialiseerde, geprimede pomp")
        spr = self.pump.cfg.steps_rev
        phase_steps = round(phase*spr) % spr
        steps = (phase_steps-self.pump.position % spr) % spr
        if not steps:
            return None
        started = time.perf_counter()
        record = self.begin("PHASE_SHIFT", steps, metadata={"requested_phase_rev": phase,
                           "excluded_from_dose": True})
        try:
            self.progress_sink("Faseverplaatsing vooruit; opbrengst buiten meetdosering")
            self.pump.move(steps)
            record.update(status="completed", end_position_steps=self.pump.position)
            return record
        except Exception as exc:
            record.update(status="failed", error=str(exc))
            raise
        finally:
            self.finish(record, started)

    def calibrate(self, revolutions, repeats):
        revolutions = integer(revolutions, "Kalibratie-omwentelingen", 10000)
        repeats = integer(repeats, "Herhalingen", 100)
        steps = self.pump.cfg.steps(revolutions)
        rows = []
        for repeat in range(1, repeats+1):
            self.progress_sink(f"Kalibratie {repeat}/{repeats}")
            rows.append(self.shot(steps, metadata={"repeat": repeat}))
        gpr = sum(r["mass_g"] for r in rows)/sum(r["rev"] for r in rows)
        values = [r["g_per_rev"] for r in rows]
        cv = 100*statistics.stdev(values)/gpr if repeats > 1 else None
        density = self.pump.cfg.density
        self.pump.check()
        return {"schema": "ceramic-pump-calibration-v1", "created": datetime.now().astimezone().isoformat(),
                "context": self.pump.cfg.context(), "policy": self.policy.context(), "g_per_rev": gpr,
                "ml_per_rev": gpr/density if density else None, "cv_pct": cv, "samples": rows}

    def dose(self, profile, target, unit="g", verify=True, metadata=None):
        validate_profile(profile, self.pump.cfg)
        target = positive(target, "Doseerdoel")
        if unit not in ("g", "mL"):
            raise ValueError("Doseereenheid moet g of mL zijn")
        density = self.pump.cfg.density
        if unit == "mL" and density is None:
            raise ValueError("Voor mL moet dichtheid bekend zijn")
        target_g = target if unit == "g" else target*density
        steps = self.pump.cfg.steps(target_g/profile["g_per_rev"])
        if steps % self.pump.cfg.steps_rev and not verify:
            raise ValueError("Deeldosering vereist weging; lineaire opbrengst binnen omwenteling onbekend")
        metadata = {"target_basis": "whole-revolution estimate; fractional output measured independently", **(metadata or {})}
        if verify:
            return self.shot(steps, target_g, "DOSE", metadata)
        if not self.pump.primed:
            raise RuntimeError("Eerst primen")
        started = time.perf_counter()
        record = self.begin("DOSE", steps, target_g, metadata)
        try:
            self.pump.move(steps)
            record["status"] = "unverified"
            return record
        except Exception as exc:
            record.update(status="failed", error=str(exc))
            raise
        finally:
            self.finish(record, started)

    def plan(self, revolutions, repeats, rpms=None, phases=None, reference_rev=None, blank_s=None):
        """Validate full experiment before any movement; include every phase shift."""
        counts = [positive(v, "Testomwentelingen") for v in revolutions]
        repeats = integer(repeats, "Herhalingen", 100)
        rpms = [positive(v, "Toerental") for v in rpms] if rpms is not None else [self.pump.cfg.speed*60/self.pump.cfg.steps_rev]
        phases = [bounded_phase(v) for v in phases] if phases is not None else [None]
        if not counts or not rpms or not phases or any(v > 10000 for v in counts):
            raise ValueError("Lege testmatrix of meer dan 10000 omwentelingen per meting")
        if reference_rev is not None:
            reference_rev = integer(reference_rev, "Referentieomwentelingen", 10000)
        blank_s = positive(blank_s, "Blancoduur") if blank_s is not None else None
        n = len(rpms)*(sum((len(phases) if not float(v).is_integer() else 1)*repeats for v in counts) + (len(counts)+1 if reference_rev else 0))
        if n > 1000:
            raise ValueError("Maximaal 1000 doseringen inclusief referenties")
        position = self.pump.position
        if position is None:
            raise ValueError("Eerst initialiseren")
        rows = []
        for rpm in rpms:
            speed = integer(round(rpm*self.pump.cfg.steps_rev/60), "Snelheid")
            def add(kind, rev, phase=None, repeat=None):
                nonlocal position
                steps = self.pump.cfg.steps(rev)
                shift = (round(phase*self.pump.cfg.steps_rev) % self.pump.cfg.steps_rev - position % self.pump.cfg.steps_rev) % self.pump.cfg.steps_rev if phase is not None else 0
                position += shift + steps
                if position > 2147483647:
                    raise ValueError("Testmatrix overschrijdt positieteller")
                rows.append({"kind": kind, "requested_rev": rev, "steps": steps, "phase": phase,
                             "phase_shift_steps": shift, "repeat": repeat, "speed": speed,
                             "requested_rpm": rpm, "rpm": speed*60/self.pump.cfg.steps_rev})
            if reference_rev:
                add("REFERENCE", reference_rev)
            for count in counts:
                for phase in phases if not float(count).is_integer() else [None]:
                    for repeat in range(1, repeats+1):
                        add("PROTOCOL", count, phase, repeat)
                if reference_rev:
                    add("REFERENCE", reference_rev)
        # Estimate, not a guarantee: settling, scale IO and disk IO may extend run.
        expected_s = sum((r["steps"]+r["phase_shift_steps"])/r["speed"] + 0.35 + 2*self.policy.window_s + self.policy.settle_min_s + r["speed"]/self.pump.cfg.accel + r["speed"]/self.pump.cfg.decel + (0.35 if r["phase_shift_steps"] else 0) for r in rows) + (blank_s or 0)
        return {"rows": rows, "dose_count": len(rows), "phase_shift_count": sum(r["phase_shift_steps"] > 0 for r in rows),
                "total_rev": sum((r["steps"]+r["phase_shift_steps"])/self.pump.cfg.steps_rev for r in rows),
                "estimated_s": expected_s, "blank_s": blank_s}

    def protocol(self, revolutions, repeats, rpms=None, phases=None, reference_rev=None, blank_s=None):
        plan = self.plan(revolutions, repeats, rpms, phases, reference_rev, blank_s)
        original_speed = self.pump.cfg.speed
        self.journal("plan", plan)
        try:
            if plan["blank_s"]:
                self.blank(plan["blank_s"])
            for index, row in enumerate(plan["rows"], 1):
                self.pump.check()
                self.pump.cfg.speed = row["speed"]
                prefix = f"Meting {index}/{len(plan['rows'])} · {row['rpm']:g} rpm · {row['requested_rev']:g} omw"
                self.progress_sink(prefix)
                if row["phase"] is not None:
                    self.phase_shift(row["phase"])
                old_sink = self.progress_sink
                self.progress_sink = lambda stage, p=prefix, sink=old_sink: sink(f"{p} — {stage}")
                try:
                    self.shot(row["steps"], kind=row["kind"], metadata={"requested_rev": row["requested_rev"],
                              "repeat": row["repeat"], "requested_phase_rev": row["phase"],
                              "requested_rpm": row["requested_rpm"], "sequence": index, "total": len(plan["rows"])})
                finally:
                    self.progress_sink = old_sink
            return self.analysis()
        finally:
            self.pump.cfg.speed = original_speed
            self.save_results()

    def analysis(self):
        groups = {}
        for row in self.records:
            if row.get("status") != "completed" or row["kind"] not in ("PROTOCOL", "REFERENCE", "CALIBRATION", "DOSE"):
                continue
            spr = row["context"]["steps_rev"]
            phase = round(row["start_phase_rev"], 9) if row["steps"] % spr else None
            key = (row["kind"], row["steps"], spr, row["context"]["speed"], phase)
            groups.setdefault(key, []).append(row)
        summaries = []
        for (kind, steps, spr, speed, phase), rows in groups.items():
            values = [row["mass_g"] for row in rows]
            mean = statistics.mean(values)
            sd = statistics.stdev(values) if len(values) > 1 else None
            for row in rows:
                row["deviation_from_group_mean_g"] = row["mass_g"]-mean
                row["deviation_from_group_mean_pct"] = 100*(row["mass_g"]-mean)/mean if mean else None
            summaries.append({"kind": kind, "steps": steps, "rev": rows[0]["rev"], "speed": speed,
                              "rpm": rows[0]["rpm"], "start_phase_rev": phase, "n": len(rows),
                              "mean_g": mean, "sd_g": sd, "range_g": max(values)-min(values),
                              "cv_pct": 100*sd/mean if sd is not None and mean else None,
                              "mean_ml": mean/rows[0]["context"]["density"] if rows[0]["context"]["density"] else None,
                              "mean_error_g": statistics.mean(r["error_g"] for r in rows) if all("error_g" in r for r in rows) else None,
                              "mean_error_pct": statistics.mean(r["error_pct"] for r in rows) if all("error_pct" in r for r in rows) else None,
                              "phase_dependent": steps % spr != 0})
        return summaries

    def save_results(self):
        self.directory.mkdir(parents=True, exist_ok=True)
        payload = {"schema": "ceramic-pump-results-v3", "session_id": self.session_id,
                   "context": self.pump.cfg.context(), "policy": self.policy.context(),
                   "records": self.records, "scale_samples": self.scale_samples, "analysis": self.analysis(),
                   "phase_note": "Phase relative to software command counter; physical piston angle unknown. Phase-shift fluid excluded from dose, retained in vessel. Fractional groups split by phase and speed."}
        atomic_text(self.path, json.dumps(payload, indent=2, ensure_ascii=False, allow_nan=False))
        flat = [{**row.get("context", {}), **{k:(json.dumps(v, ensure_ascii=False) if isinstance(v, (dict, list)) else v) for k,v in row.items() if k != "context"}} for row in self.records]
        for suffix, rows in (("", flat), ("_analysis", payload["analysis"]), ("_scale", self.scale_samples)):
            columns = list(dict.fromkeys(key for row in rows for key in row)) or ["status"]
            output = io.StringIO(newline="")
            writer = csv.DictWriter(output, fieldnames=columns)
            writer.writeheader()
            writer.writerows(rows)
            atomic_text(self.path.with_name(self.path.stem+suffix+".csv"), output.getvalue())
        return self.path

def validate_profile(profile, config):
    if not isinstance(profile, dict) or profile.get("schema") != "ceramic-pump-calibration-v1":
        raise ValueError("Geen ceramic-pump-kalibratieprofiel")
    if profile.get("context") != config.context():
        raise ValueError("Kalibratie past niet bij pomp, vloeistof of bewegingsinstellingen")
    gpr = profile.get("g_per_rev")
    if isinstance(gpr, bool) or not isinstance(gpr, (float, int)):
        raise ValueError("g/rev moet een getal zijn")
    positive(gpr, "g/rev")
    ml = profile.get("ml_per_rev")
    if config.density is None:
        if ml is not None:
            raise ValueError("mL/rev zonder dichtheid")
    elif ml is None or not math.isclose(positive(ml, "mL/rev"), gpr / config.density, rel_tol=1e-9):
        raise ValueError("mL/rev past niet bij g/rev en dichtheid")
    cv = profile.get("cv_pct")
    if cv is not None and (not isinstance(cv, (int, float)) or not math.isfinite(cv) or cv < 0):
        raise ValueError("Ongeldige kalibratiespreiding")


def save_profile(profile, directory):
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / f"cal_{safe_name(profile['context']['fluid'])}_{datetime.now():%Y%m%d_%H%M%S_%f}.json"
    path.write_text(json.dumps(profile, indent=2, ensure_ascii=False, allow_nan=False), encoding="utf-8")
    return path


def analysis_for_records(records):
    holder = object.__new__(Measurements)
    holder.records = records
    return holder.analysis()


def load_results(path):
    """Read exports/journals for inspection only. Never resume motor commands."""
    path = Path(path)
    if path.suffix.lower() == ".jsonl":
        lines = path.read_text(encoding="utf-8-sig").splitlines()
        records, samples, context = {}, [], {}
        for index, line in enumerate(lines):
            if not line.strip():
                continue
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                if index == len(lines)-1:
                    break  # A crash may leave only the very last line unfinished.
                raise ValueError(f"Beschadigd journal op regel {index+1}")
            payload = event.get("payload", {})
            if event.get("event") in ("measurement_started", "measurement_finished", "measurement_updated"):
                records[payload["measurement_id"]] = payload
                context = payload.get("context", context)
            elif event.get("event") == "scale_sample":
                samples.append(payload)
        result = {"schema": "ceramic-pump-results-v3", "records": list(records.values()),
                  "scale_samples": samples, "context": context, "recovered_from": str(path)}
    else:
        result = json.loads(path.read_text(encoding="utf-8-sig"))
        if isinstance(result, list):
            result = {"records": result, "scale_samples": []}
    if not isinstance(result, dict) or not isinstance(result.get("records"), list):
        raise ValueError("Geen meetresultatenbestand")
    for row in result["records"]:
        if row.get("status") == "running":
            row["status"] = "interrupted_unknown_motion"
        # Backward-compatible display of earlier v2 records.
        row.setdefault("rpm", row.get("context", {}).get("speed", 0)*60/max(row.get("context", {}).get("steps_rev", 1), 1))
    result["analysis"] = analysis_for_records(result["records"])
    result.setdefault("scale_samples", [])
    return result
