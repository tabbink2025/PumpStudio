"""VSD pump boards over CAN (IXXAT via python-can): same VWA frames as Camelot CVWCanVCI.

Request: [0x00, rec 8..2, drive, 0xBB, param LE32] for params 8..2, then [cmd, 1, drive, 0xBB, param1 LE32].
Reply:   [rsp, rec, drive, 0xAA, value LE32]; rec 1 completes a reply, rec | 0x80 marks an error.
CAN id = board node (610..639), standard frames, 250 kbit/s. FF FF 00 .. AA is a heartbeat.

Line-delimited JSON like hardware_bridge. 'send' only transmits (several drives can be started back to
back, as Camelot does for a parallel dose); 'wait' blocks until each listed drive answered after its
last send, so pumps really run at the same time. 'stop' sends INIT (70) to every drive used.
"""
import argparse
import json
import sys
import threading
import time
from datetime import datetime
from pathlib import Path

INIT = 70
UPDATE = 1


class Boards:
    def __init__(self, output, bus_factory=None):
        self.output = Path(output)
        self.output.mkdir(parents=True, exist_ok=True)
        self.log = self.output / ("vsd_can_" + datetime.now().strftime("%Y%m%d_%H%M%S_%f") + ".jsonl")
        self.log_lock = threading.Lock()
        self.bus_factory = bus_factory
        self.bus = None
        self.receiver = None
        self.receiver_stop = threading.Event()
        self.cond = threading.Condition()
        self.regs = {}      # (node, drive) -> {rec: value}
        # (node, drive) -> complete replies (rec-1 frames) in arrival order, bounded. A board can send an error-flagged
        # register set right after its "done" (seen on the machine, board 626): `wait` takes the FIRST reply after
        # the send that is accepted or an error, so done followed by such a set is a success (as Camelot).
        self.replies = {}
        self.err_regs = {}  # (node, drive) -> {rec: value} of error-flagged frames (Camelot keeps them apart)
        self.sent = {}      # (node, drive) -> monotonic time of last command
        self.active = set()
        self.send_lock = threading.Lock()
        self.last_node = None
        self.fault = None
        # STOP generation: 'stop' bumps it; a wait for drives last sent before that STOP ends at once.
        self.stop_gen = 0
        self.sent_gen = {}  # (node, drive) -> stop generation at its last send

    def event(self, kind, node=None, data=b"", info=""):
        row = dict(time=datetime.now().astimezone().isoformat(), kind=kind, node=node, data=bytes(data).hex(), info=str(info))
        with self.log_lock, self.log.open("a", encoding="utf-8") as stream:
            stream.write(json.dumps(row, ensure_ascii=False) + "\n")

    def connect(self, args):
        if self.bus:
            raise RuntimeError("CAN al verbonden")
        if self.bus_factory:
            self.bus = self.bus_factory(args)
        else:
            import can
            self.bus = can.Bus(interface=args.get("interface", "ixxat"), channel=int(args.get("channel", 0)),
                               bitrate=int(args.get("bitrate", 250000)), rx_fifo_size=1024)
        self.fault = None
        self.receiver_stop.clear()
        self.receiver = threading.Thread(target=self.receive, daemon=True)
        self.receiver.start()
        return dict(connected=True, log=str(self.log))

    def receive(self):
        try:
            while not self.receiver_stop.is_set():
                msg = self.bus.recv(timeout=0.1)
                if msg is None or msg.is_error_frame or msg.is_remote_frame or len(msg.data) != 8:
                    continue
                data = bytes(msg.data)
                if data[3] != 0xAA:
                    continue
                if data[0] == 0xFF and data[1] == 0xFF:
                    continue  # heartbeat
                self.event("RX", msg.arbitration_id, data)
                key = (msg.arbitration_id, data[2])
                rec = data[1] & 0x7F
                flagged = bool(data[1] & 0x80)
                value = int.from_bytes(data[4:8], "little", signed=True)
                with self.cond:
                    # Error-flagged registers never overwrite the normal ones (the reply that came before stays intact).
                    regs = (self.err_regs if flagged else self.regs).setdefault(key, {})
                    regs[rec] = value
                    if rec == 1:
                        code = 0xFFFF if data[0] == 0xFF else data[0]
                        reply = dict(t=time.monotonic(), code=code, error=flagged or code == 0xFFFF, params=[regs.get(i, 0) for i in range(1, 9)])
                        if reply["error"]:
                            self.event("ERROR_SET", key[0], data, info=dict(drive=key[1], registers=reply["params"]))
                        lst = self.replies.setdefault(key, [])
                        lst.append(reply)
                        if len(lst) > 64:
                            del lst[:len(lst) - 64]
                        self.cond.notify_all()
        except Exception as exc:  # bus gone: every waiter must see it
            self.fault = str(exc)
            self.event("RX_ERROR", info=exc)
            with self.cond:
                self.cond.notify_all()

    def send(self, args):
        if not self.bus:
            raise RuntimeError("CAN niet verbonden")
        if self.fault:
            raise RuntimeError("CAN-fout: " + self.fault)
        import can
        node, drive, cmd = int(args["node"]), int(args["drive"]), int(args["cmd"])
        params = [int(p) for p in args.get("params", [])] + [0] * 8
        if not (1 <= drive <= 8 and 0 < node < 2048 and 0 < cmd < 256):
            raise ValueError("Ongeldig CAN-adres of opdracht")
        with self.send_lock:
            # Camelot: 50 ms before a command to the same board, 10 ms otherwise.
            time.sleep(0.05 if node == self.last_node else 0.01)
            frames = [bytes([0, rec, drive, 0xBB]) + params[rec - 1].to_bytes(4, "little", signed=True) for rec in range(8, 1, -1)]
            frames.append(bytes([cmd, 1, drive, 0xBB]) + params[0].to_bytes(4, "little", signed=True))
            for frame in frames:
                self.bus.send(can.Message(arbitration_id=node, data=frame, is_extended_id=False))
                self.event("TX", node, frame)
            self.last_node = node
            with self.cond:
                self.sent[(node, drive)] = time.monotonic()
                self.sent_gen[(node, drive)] = self.stop_gen
            self.active.add((node, drive))
        return dict(sent=True)

    def wait(self, args):
        """Block until every target answered after its last send with an accepted code.
        Update replies (1) never complete; an empty code list accepts any other reply."""
        targets = args["targets"]
        deadline = time.monotonic() + float(args.get("timeout", 30))
        results = {}
        stopped = False
        with self.cond:
            while True:
                keys = [(int(t["node"]), int(t["drive"])) for t in targets]
                if any(self.sent_gen.get(k, self.stop_gen) < self.stop_gen for k in keys if k not in results):
                    stopped = True
                    break
                for t in targets:
                    key = (int(t["node"]), int(t["drive"]))
                    if key in results:
                        continue
                    codes = t.get("codes") or []
                    since = self.sent.get(key, 0)
                    for reply in self.replies.get(key, []):
                        if reply["t"] < since:
                            continue
                        if reply["error"] or (reply["code"] in codes if codes else reply["code"] != UPDATE):
                            results[key] = dict(node=key[0], drive=key[1], code=reply["code"], error=reply["error"],
                                                position=reply["params"][0], params=reply["params"])
                            break
                if len(results) == len(targets) or self.fault:
                    break
                left = deadline - time.monotonic()
                if left <= 0:
                    break
                self.cond.wait(min(left, 0.1))
        missing = [dict(node=int(t["node"]), drive=int(t["drive"])) for t in targets if (int(t["node"]), int(t["drive"])) not in results]
        return dict(results=list(results.values()), missing=missing, fault=self.fault, stopped=stopped)

    def stop(self):
        """Camelot abort path: INIT to every drive that was commanded."""
        with self.cond:
            self.stop_gen += 1
            self.cond.notify_all()
        if not self.bus:
            return dict(stopped=False)
        for node, drive in sorted(self.active):
            try:
                self.send(dict(node=node, drive=drive, cmd=INIT, params=[]))
            except Exception as exc:
                self.event("STOP_ERROR", node, info=exc)
        return dict(stopped=True, drives=len(self.active))

    def disconnect(self):
        self.receiver_stop.set()
        if self.receiver:
            self.receiver.join(timeout=2)
        if self.bus:
            try:
                self.bus.shutdown()
            except Exception:
                pass
        self.bus = None
        self.active.clear()
        return dict(connected=False)

    def scan(self, args):
        """Listen on every IXXAT channel (or the given ones) without sending anything.
        Reports frames, error frames and boards that send the FF FF 00 .. AA heartbeat."""
        if self.bus:
            raise RuntimeError("Eerst CAN verbreken; scannen opent de kanalen zelf")
        import can
        seconds, bitrate = float(args.get("seconds", 12)), int(args.get("bitrate", 250000))
        channels = args.get("channels")
        found = []
        if channels is None:
            try:
                found = can.detect_available_configs(interfaces=["ixxat"])
            except Exception as exc:
                self.event("SCAN_ERROR", info=exc)
            channels = sorted({int(c.get("channel", 0)) for c in found}) or [0]
        result = []
        for ch in channels:
            row = dict(channel=ch, frames=0, errors=0, nodes={}, heartbeat=[], error=None)
            try:
                bus = self.bus_factory(dict(channel=ch)) if self.bus_factory else can.Bus(interface="ixxat", channel=int(ch), bitrate=bitrate, rx_fifo_size=1024)
            except Exception as exc:
                row["error"] = str(exc)
                result.append(row)
                continue
            end = time.monotonic() + seconds
            try:
                while time.monotonic() < end:
                    msg = bus.recv(timeout=0.5)
                    if msg is None:
                        continue
                    if msg.is_error_frame:
                        row["errors"] += 1
                        continue
                    row["frames"] += 1
                    node = msg.arbitration_id
                    row["nodes"][str(node)] = row["nodes"].get(str(node), 0) + 1
                    d = bytes(msg.data)
                    if len(d) >= 4 and d[0] == 0xFF and d[1] == 0xFF and d[3] == 0xAA and node not in row["heartbeat"]:
                        row["heartbeat"].append(node)
            except Exception as exc:
                row["error"] = str(exc)
            finally:
                bus.shutdown()
            row["heartbeat"].sort()
            self.event("SCAN", info=row)
            result.append(row)
        return dict(bitrate=bitrate, seconds=seconds, hardware=[c.get("unique_hardware_id") for c in found], channels=result)

    def dispatch(self, op, args):
        if op == "scan":
            return self.scan(args)
        if op == "ping":
            return dict(version=1, connected=self.bus is not None)
        if op == "connect":
            return self.connect(args)
        if op == "disconnect":
            return self.disconnect()
        if op == "send":
            return self.send(args)
        if op == "wait":
            return self.wait(args)
        raise ValueError("Onbekende operatie: " + op)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, required=False)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    boards = Boards(args.output)
    out_lock = threading.Lock()

    def reply(value):
        with out_lock:
            print(json.dumps(value, ensure_ascii=False), flush=True)

    # Operations run in order on a worker thread, so this loop keeps reading stdin: a 'stop' line is handled at
    # once, also while a long 'wait' runs (the wait then ends with stopped=True).
    import queue
    jobs = queue.Queue()

    def worker():
        while True:
            item = jobs.get()
            if item is None:
                return
            i, o, a = item
            try:
                reply(dict(id=i, ok=True, result=boards.dispatch(o, a)))
            except Exception as exc:
                boards.event("OP_ERROR", info=exc)
                reply(dict(id=i, ok=False, error=str(exc)))

    runner = threading.Thread(target=worker, daemon=True)
    runner.start()
    for line in sys.stdin:
        try:
            request = json.loads(line)
            ident, op = request["id"], request["op"]
        except Exception as exc:
            reply(dict(id=None, ok=False, error=str(exc)))
            continue
        if op == "stop":
            # Out of band, like hardware_bridge: never queued behind a long wait.
            threading.Thread(target=lambda i=ident: reply(dict(id=i, ok=True, result=boards.stop())), daemon=True).start()
            continue
        jobs.put((ident, op, request.get("args", {})))
    jobs.put(None)
    runner.join(5)
    boards.disconnect()


if __name__ == "__main__":
    main()
