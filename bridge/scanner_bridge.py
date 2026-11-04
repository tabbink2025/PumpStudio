"""Barcode scanner on a COM port (pyserial), "generiek" protocol as Camelot GenericBarcodeReader.

Line-delimited JSON like the other bridges. 'read' sends the trigger-on bytes, collects the answer until
<STX>code<ETX>, <STX><NAK> or a CR/LF-terminated line, or until the time-out, then sends the trigger-off
bytes. The raw bytes go back as a list of integers; Pump Studio parses them. 'stop' ends a running read.
"""
import argparse
import json
import sys
import threading
import time

STX, ETX, NAK = 0x02, 0x03, 0x15


def complete(raw):
    """True when `raw` holds a whole answer (same rules as scanner::parse in Pump Studio)."""
    start = raw.find(bytes([STX]))
    body = raw[start + 1:] if start >= 0 else raw
    if body[:1] == bytes([NAK]) or (start < 0 and NAK in raw):
        return True
    return any(c in body for c in (ETX, 0x0D, 0x0A))


class Scanner:
    def __init__(self, serial_factory=None):
        self.serial_factory = serial_factory
        self.port = None
        self.stopping = threading.Event()

    def connect(self, args):
        if self.serial_factory:
            self.port = self.serial_factory(args)
        else:
            import serial
            self.port = serial.Serial(args["port"], int(args.get("baud", 9600)), timeout=0.05)
        return dict(connected=True)

    def read(self, args):
        if not self.port:
            raise RuntimeError("Scanner niet verbonden")
        self.stopping.clear()
        self.port.reset_input_buffer()
        self.port.write(bytes(args.get("on", [])))
        deadline = time.monotonic() + float(args.get("timeout", 10))
        raw = b""
        try:
            while time.monotonic() < deadline and not self.stopping.is_set():
                chunk = self.port.read(64)
                if chunk:
                    raw += chunk
                    if complete(raw):
                        break
        finally:
            off = bytes(args.get("off", []))
            if off:
                self.port.write(off)
        return dict(bytes=list(raw), stopped=self.stopping.is_set())

    def stop(self):
        self.stopping.set()
        return dict(stopped=True)

    def disconnect(self):
        if self.port:
            try:
                self.port.close()
            except Exception:
                pass
        self.port = None
        return dict(connected=False)

    def dispatch(self, op, args):
        if op == "ping":
            return dict(version=1, connected=self.port is not None)
        if op == "connect":
            return self.connect(args)
        if op == "read":
            return self.read(args)
        if op == "disconnect":
            return self.disconnect()
        raise ValueError("Onbekende operatie: " + op)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", required=False)
    parser.add_argument("--output", required=False)
    parser.parse_args()
    scanner = Scanner()
    out_lock = threading.Lock()

    def reply(value):
        with out_lock:
            print(json.dumps(value), flush=True)

    for line in sys.stdin:
        try:
            request = json.loads(line)
            ident, op = request["id"], request["op"]
        except Exception as exc:
            reply(dict(id=None, ok=False, error=str(exc)))
            continue
        if op == "stop":
            reply(dict(id=ident, ok=True, result=scanner.stop()))
            continue

        def work(i=ident, o=op, a=request.get("args", {})):
            try:
                reply(dict(id=i, ok=True, result=scanner.dispatch(o, a)))
            except Exception as exc:
                reply(dict(id=i, ok=False, error=str(exc)))
        # Reads run on their own thread so a 'stop' line is read at once.
        threading.Thread(target=work, daemon=True).start() if op == "read" else work()
    scanner.disconnect()


if __name__ == "__main__":
    main()
