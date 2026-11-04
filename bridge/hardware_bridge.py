"""Line-delimited JSON transport only. Pump workflow and curve inversion live in Rust.

Reuses existing Python CAN encoder, IXXAT driver and scale parser unchanged.
No connection or hardware command is sent until an explicit connect request.
"""
import argparse
import json
import queue
import sys
import threading
import time
from datetime import datetime
from pathlib import Path


class Hardware:
    def __init__(self, repo, output):
        self.repo, self.output = repo, output
        self.transport = self.adapter = self.scale = self.measurement = None
        self.scales = {}  # head index (0 = LH1, 1 = LH2) -> (scale, measurement); scale/measurement = active head
        self.active_head = 0
        self.receiver_stop = threading.Event()
        self.receiver = None
        self.stop_pending = threading.Event()
        self.output.mkdir(parents=True, exist_ok=True)
        self.log_lock = threading.Lock()
        self.log = self.output / ("hardware_" + datetime.now().strftime("%Y%m%d_%H%M%S_%f") + ".jsonl")

    def event(self, kind, node=None, data=b"", info=""):
        row = dict(time=datetime.now().astimezone().isoformat(), kind=kind, node=node,
                   data=bytes(data).hex(), info=str(info))
        with self.log_lock, self.log.open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(row, ensure_ascii=False) + "\n")
            stream.flush()

    def connect(self, cfg):
        if self.transport:
            raise RuntimeError("Hardware al verbonden")
        own = Path(__file__).resolve().parent / "vendor"
        sys.path[:0] = [str(own), str(self.repo / "tools"), str(self.repo / "src"), str(self.repo / "backend")]
        import can
        from types import SimpleNamespace
        from pump_debug_legacy import PumpDebugApp
        from pump_counter import CommandCounterAdapter
        from ceramic_pump import PumpConfig, Measurements, MeasurementPolicy
        from utils.scale_communication import ScaleCommunicator

        class Transport:
            _sequence = PumpDebugApp._sequence
            _send_frame = PumpDebugApp._send_frame
            _track_feedback = PumpDebugApp._track_feedback
            _fo_move_forward = PumpDebugApp._fo_move_forward
            _fo_check_cancel = PumpDebugApp._fo_check_cancel

        t = Transport()
        t.bus = None
        t.fo_cancel = threading.Event()
        t.send_lock = threading.Lock()
        t.commanded = {i: 0 for i in range(1, 9)}
        t.reported = {}
        t.move_wait = None
        t.move_done = threading.Event()
        t.move_done_pos = None
        t.counter_valid = False
        t.counter_bus = t.counter_context = None
        t.wait_done_var = SimpleNamespace(get=lambda: cfg["require_feedback"])
        t._event = self.event
        self.transport = t
        self.cfg = cfg
        self.stop_pending.clear()
        try:
            pump_cfg = PumpConfig(**{key: cfg[key] for key in ("pump_type", "steps_rev", "node", "motor", "speed", "accel", "decel", "fluid", "stroke_setting")})
            t.bus = can.Bus(interface="ixxat", channel=cfg["channel"], bitrate=cfg["bitrate"], rx_fifo_size=1024)
            self.adapter = CommandCounterAdapter(t, pump_cfg)
            self.policy = MeasurementPolicy(window_s=cfg["window_s"], range_g=cfg["range_g"], timeout_s=cfg["timeout_s"], settle_min_s=max(cfg["settle_s"], 0.001))
            if cfg.get("use_scale", True):
                self.open_scale(cfg)
            self.settle_s = cfg["settle_s"]
            self.receiver_stop.clear()
            def receive():
                try:
                    while not self.receiver_stop.is_set():
                        msg = t.bus.recv(timeout=0.1)
                        if msg is None:
                            continue
                        self.event("RX", msg.arbitration_id, msg.data)
                        if msg.is_error_frame:
                            raise RuntimeError("CAN error frame")
                        if not msg.is_extended_id and not msg.is_remote_frame:
                            t._track_feedback(msg.arbitration_id, bytes(msg.data))
                except Exception as exc:
                    self.adapter.fault = str(exc)
                    self.adapter.reference_valid = False
                    t.counter_valid = False
                    self.event("RX_ERROR", info=exc)
            self.receiver = threading.Thread(target=receive, daemon=True)
            self.receiver.start()
            return dict(connected=True, audit_log=str(self.log))
        except Exception:
            self.disconnect()
            raise

    def stop(self):
        self.stop_pending.set()
        if self.transport:
            self.transport.fo_cancel.set()
        if self.adapter:
            self.adapter.abort()  # Original INIT emergency path, no new frames.

    def disconnect(self):
        self.receiver_stop.set()
        if self.transport:
            self.transport.fo_cancel.set()
        if self.receiver:
            self.receiver.join(timeout=2)
        if self.adapter:
            self.adapter.restore()
        self.close_scale()
        if self.transport and self.transport.bus:
            self.transport.bus.shutdown()
        self.adapter = self.transport = self.scale = self.measurement = None
        return dict(connected=False)

    def open_scale(self, cfg):
        """Scale is optional: without it the pump can still be moved and configured.
        COM port or network (existing ScaleCommunicator.connect_tcp, MT-SICS over raw TCP).
        Network with scale_dual: LH1 and LH2 scales are both connected; measure_head picks the one weighed."""
        from ceramic_pump import Measurements
        from utils.scale_communication import ScaleCommunicator
        self.close_scale()
        def measurement(scale):
            return Measurements(self.adapter, scale, threading.Lock(),
                                lambda kind, details: self.event(kind, info=details), self.output, policy=self.policy)
        if cfg.get("scale_tcp"):
            targets = [(0, str(cfg["scale_host"]).strip(), int(cfg["scale_tcp_port"]))]
            if cfg.get("scale_dual"):
                targets.append((1, str(cfg["scale_host2"]).strip(), int(cfg["scale_tcp_port2"])))
            failed = []
            for head, host, tcp_port in targets:
                scale = ScaleCommunicator()
                if scale.connect_tcp(host, tcp_port):
                    self.scales[head] = (scale, measurement(scale))
                else:
                    failed.append(f"LH{head + 1} {host}:{tcp_port}")
            if failed:
                self.close_scale()
                raise RuntimeError("Weegschaal niet verbonden: " + ", ".join(failed) + "; netwerk en IP-adres controleren")
            self.select_head(int(cfg.get("measure_head", 0)) if cfg.get("scale_dual") else 0)
            return
        scale = ScaleCommunicator()
        if not scale.connect(cfg["port"], cfg["baud"]):
            try:
                from serial.tools import list_ports
                ports = ", ".join(sorted(p.device for p in list_ports.comports())) or "geen"
            except Exception:
                ports = "onbekend"
            raise RuntimeError(f"Weegschaal op {cfg['port']} ({cfg['baud']} baud) niet verbonden: poort bestaat niet, "
                               f"is bezet (Camelot, Omron-service, andere app) of weegschaal staat uit. Aanwezige COM-poorten: {ports}. "
                               "Netwerkweegschaal? Kies bij Aansluiting 'Netwerk (TCP/IP)'.")
        self.scales[0] = (scale, measurement(scale))
        self.select_head(0)

    def select_head(self, head):
        if head not in self.scales:
            raise RuntimeError(f"Geen weegschaal LH{head + 1} verbonden")
        self.active_head = head
        self.scale, self.measurement = self.scales[head]

    def close_scale(self):
        for scale, _ in self.scales.values():
            try:
                scale.disconnect()
            except Exception as exc:
                self.event("SCALE_CLOSE_ERROR", info=exc)
        self.scales = {}
        self.scale = self.measurement = None

    def configure(self, cfg):
        """Live pump/motion/weighing settings; validated completely before anything changes."""
        from ceramic_pump import PumpConfig, MeasurementPolicy
        for key in ("channel", "bitrate"):
            if cfg[key] != self.cfg[key]:
                raise RuntimeError("Verbindingsinstelling alleen na opnieuw verbinden: " + key)
        keys = ("pump_type", "steps_rev", "node", "motor", "speed", "accel", "decel", "fluid", "stroke_setting")
        checked = PumpConfig(**{key: cfg[key] for key in keys})
        policy = MeasurementPolicy(window_s=cfg["window_s"], range_g=cfg["range_g"], timeout_s=cfg["timeout_s"],
                                   settle_min_s=max(cfg["settle_s"], 0.001))
        a = self.adapter
        pump_changed = any(getattr(checked, key) != getattr(a.cfg, key) for key in ("node", "motor", "pump_type", "steps_rev"))
        # Mutate in place: measurement and counter code keep references to this config object.
        # Node/motor select another pump on the same CAN bus; its counter needs a new ZERO/HOME.
        for key in ("node", "response_node", "motor", "pump_type", "steps_rev", "speed", "accel", "decel", "fluid", "stroke_setting"):
            setattr(a.cfg, key, getattr(checked, key))
        self.policy = policy
        for _, m in self.scales.values():
            m.policy = policy
        self.settle_s = cfg["settle_s"]
        a.require_feedback = bool(cfg["require_feedback"])
        scale_keys = ("use_scale", "port", "baud", "scale_tcp", "scale_host", "scale_tcp_port", "scale_dual", "scale_host2", "scale_tcp_port2")
        old_scale = {key: self.cfg.get(key, True if key == "use_scale" else None) for key in scale_keys}
        self.cfg.update(cfg)  # wait_done_var reads this dict
        if pump_changed:
            a.reference_valid = False
            self.transport.counter_valid = False
        scale_error = None
        if {key: cfg.get(key) for key in scale_keys} != old_scale or (cfg.get("use_scale", True) and not self.scale):
            # Reconnect scale only; a failure is reported to Rust instead of raised so the pump reference survives.
            self.close_scale()
            if cfg.get("use_scale", True):
                try:
                    self.open_scale(cfg)
                except Exception as exc:
                    self.close_scale()
                    scale_error = str(exc)
        self.event("CONFIGURE", info={key: cfg.get(key) for key in keys + ("window_s", "range_g", "timeout_s", "settle_s", "require_feedback") + scale_keys})
        result = self.dispatch("status", {})
        if self.scales and cfg.get("scale_dual") and int(cfg.get("measure_head", 0)) in self.scales:
            self.select_head(int(cfg.get("measure_head", 0)))
        result.update(scale_connected=self.scale is not None, scale_error=scale_error, measure_head=self.active_head)
        return result

    def dispatch(self, op, args):
        if op == "ping":
            return dict(version=1, hardware_open=self.transport is not None)
        if op == "connect":
            return self.connect(args)
        if op == "disconnect":
            return self.disconnect()
        if not self.adapter:
            raise RuntimeError("Hardware niet verbonden")
        a = self.adapter
        if op == "configure":
            return self.configure(args)
        if op == "status":
            # Software state only; no READ_STATUS frame and no physical position claim.
            return dict(position=a.position, reference_valid=a.reference_valid and bool(self.transport.counter_valid),
                        homing_status=a.homing_status, fault=a.fault, cancelled=a.cancel.is_set())
        if op in ("home", "zero"):
            a.cancel.clear()
            # A failed CAN receiver requires reconnecting, never hide its fault.
            a.check()
            self.stop_pending.clear()
            if op == "zero":
                a.establish_zero()
            else:
                from pump_counter import position_binding
                a.home()
                self.transport.counter_valid = True
                self.transport.counter_bus = self.transport.bus
                self.transport.counter_context = position_binding(a.cfg)
            # A stop during ZERO/HOME must never restore a valid reference.
            if self.stop_pending.is_set():
                a.reference_valid = self.transport.counter_valid = False
                raise RuntimeError("Refereren geannuleerd")
            return self.dispatch("status", {})
        a.check_reference()
        if op == "rebase":
            # Controller counter limit: ZERO at standstill with confirmed ZERO_DONE (existing frames);
            # Rust keeps the continuous count as offset. No motion is commanded.
            from pump_counter import position_binding
            from pump_debug_characterization import CharacterizationAdapter
            CharacterizationAdapter.set_relative_reference(a)
            a.app.counter_valid = True
            a.app.counter_bus = a.app.bus
            a.app.counter_context = position_binding(a.cfg)
            if self.stop_pending.is_set():
                a.reference_valid = a.app.counter_valid = False
                raise RuntimeError("Tellerreset geannuleerd")
            return self.dispatch("status", {})
        if op == "move":
            a.move_absolute(args["target"])
            a.sleep(self.settle_s)
            result = self.dispatch("status", {})
            result["motion"] = a.last_motion_feedback
            return result
        if op in ("tare", "weight") and not self.measurement:
            raise RuntimeError("Geen weegschaal verbonden")
        if op in ("tare", "weight") and args.get("head") is not None and int(args["head"]) != self.active_head:
            # Reading the other head (both shown in the app); measurements keep using the active head.
            scale, m = self.scales.get(int(args["head"]), (None, None))
            if not m:
                raise RuntimeError(f"Geen weegschaal LH{int(args['head']) + 1} verbonden")
            if op == "tare":
                raise RuntimeError("Tarreren alleen op de actieve meetkop")
            start = len(m.scale_samples)
            m.active_id, m.stage = "rust-" + str(time.time_ns()), "rust_requested_stable_weight_other_head"
            weight = m.stable_weight()
            return dict(grams=weight, head=int(args["head"]), scale_status="stable_and_range_checked", window=m.last_window,
                        samples=m.scale_samples[start:])
        if op == "tare":
            with self.measurement.scale_lock:
                if not self.scale.tare():
                    raise RuntimeError("Tarreren mislukt")
            a.check_reference()
            return dict(scale_status="tare_acknowledged")
        if op == "weight":
            m = self.measurement
            start = len(m.scale_samples)
            m.active_id = "rust-" + str(time.time_ns())
            m.stage = "rust_requested_stable_weight"
            weight = m.stable_weight()
            a.check_reference()
            return dict(grams=weight, head=self.active_head, scale_status="stable_and_range_checked", window=m.last_window,
                        samples=m.scale_samples[start:])
        raise ValueError("Onbekende operatie: " + op)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    hardware = Hardware(args.repo, args.output)
    requests = queue.Queue()
    output_lock = threading.Lock()
    def reply(value):
        with output_lock:
            print(json.dumps(value, ensure_ascii=False, allow_nan=False), flush=True)
    def worker():
        for request in iter(requests.get, None):
            ident = request.get("id")
            try:
                result = hardware.dispatch(request["op"], request.get("args", {}))
                reply(dict(id=ident, ok=True, result=result))
            except Exception as exc:
                hardware.event("OP_ERROR", info=exc)
                if hardware.adapter:
                    hardware.adapter.reference_valid = False
                    hardware.transport.counter_valid = False
                    try:
                        hardware.stop()
                    except Exception as stop_error:
                        hardware.event("STOP_ERROR", info=stop_error)
                reply(dict(id=ident, ok=False, error=str(exc)))
    thread = threading.Thread(target=worker, daemon=True)
    thread.start()
    try:
        for line in sys.stdin:
            try:
                request = json.loads(line)
                if not isinstance(request, dict) or "id" not in request or "op" not in request:
                    raise ValueError("Ongeldig verzoek")
                if request["op"] == "stop":
                    # Independent of work queue so a stable-weight wait can be cancelled.
                    hardware.stop_pending.set()
                    if hardware.transport:
                        hardware.transport.fo_cancel.set()
                    def emergency(ident=request["id"]):
                        try:
                            hardware.stop()
                            reply(dict(id=ident, ok=True, result=dict(stopped=True)))
                        except Exception as exc:
                            reply(dict(id=ident, ok=False, error=str(exc)))
                    threading.Thread(target=emergency, daemon=True).start()
                else:
                    requests.put(request)
            except Exception as exc:
                reply(dict(id=None, ok=False, error=str(exc)))
    finally:
        try:
            hardware.stop()
        except Exception:
            pass
        requests.put(None)
        thread.join(timeout=10)
        hardware.disconnect()


if __name__ == "__main__":
    main()
