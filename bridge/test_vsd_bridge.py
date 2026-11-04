"""Offline: VSD bridge against a simulated board on a python-can virtual bus. No hardware."""
import tempfile
import threading
import time
import unittest
from pathlib import Path

import can

from vsd_bridge import Boards


def fake_board(channel, node, stop):
    """Collects param frames per drive; on the command frame replies rec 8..2 then rec 1 with the command code."""
    bus = can.Bus(interface="virtual", channel=channel)
    regs = {}
    while not stop.is_set():
        msg = bus.recv(timeout=0.05)
        if msg is None or msg.arbitration_id != node:
            continue
        d = bytes(msg.data)
        if d[3] != 0xBB:
            continue
        drive, rec, value = d[2], d[1], int.from_bytes(d[4:8], "little", signed=True)
        regs.setdefault(drive, {})[rec] = value
        if rec == 1:
            time.sleep(0.05 * drive)  # drives finish at different times
            for r in range(8, 1, -1):
                bus.send(can.Message(arbitration_id=node, is_extended_id=False,
                                     data=bytes([0, r, drive, 0xAA]) + regs[drive].get(r, 0).to_bytes(4, "little", signed=True)))
            code = d[0] if d[0] != 99 else 0xFF
            bus.send(can.Message(arbitration_id=node, is_extended_id=False,
                                 data=bytes([code, 1 | (0x80 if code == 0xFF else 0), drive, 0xAA]) + value.to_bytes(4, "little", signed=True)))
    bus.shutdown()


class VsdBridgeTests(unittest.TestCase):
    def setUp(self):
        self.channel = "vsd-test-%d" % time.monotonic_ns()
        self.stop = threading.Event()
        self.board = threading.Thread(target=fake_board, args=(self.channel, 612, self.stop), daemon=True)
        self.board.start()
        self.tmp = tempfile.TemporaryDirectory()
        self.boards = Boards(Path(self.tmp.name), bus_factory=lambda a: can.Bus(interface="virtual", channel=self.channel))
        self.boards.connect({})

    def tearDown(self):
        self.boards.disconnect()
        self.stop.set()
        self.board.join(2)
        self.tmp.cleanup()

    def test_parallel_dispense_on_three_drives(self):
        for drive in (1, 2, 3):
            self.boards.send(dict(node=612, drive=drive, cmd=73, params=[51200 * drive, drive, 0, 51200, 51200, 51200, 0, 0]))
        out = self.boards.wait(dict(targets=[dict(node=612, drive=d, codes=[73, 2]) for d in (1, 2, 3)], timeout=5))
        self.assertEqual(out["missing"], [])
        self.assertEqual(sorted((r["drive"], r["code"], r["position"]) for r in out["results"]),
                         [(1, 73, 51200), (2, 73, 102400), (3, 73, 153600)])
        self.assertEqual(out["results"][0]["params"][3], 51200)  # rec 4 = speed echoed

    def test_error_and_timeout_are_reported(self):
        self.boards.send(dict(node=612, drive=4, cmd=99, params=[1]))
        out = self.boards.wait(dict(targets=[dict(node=612, drive=4, codes=[73]), dict(node=613, drive=1, codes=[73])], timeout=1))
        self.assertTrue(out["results"][0]["error"])
        self.assertEqual(out["missing"], [dict(node=613, drive=1)])

    def test_stop_sends_init_to_used_drives(self):
        self.boards.send(dict(node=612, drive=2, cmd=73, params=[100]))
        self.boards.wait(dict(targets=[dict(node=612, drive=2)], timeout=5))
        self.assertEqual(self.boards.stop()["drives"], 1)
        out = self.boards.wait(dict(targets=[dict(node=612, drive=2, codes=[70])], timeout=5))
        self.assertEqual(out["results"][0]["code"], 70)


    def test_blocks_outstanding_together_wait_on_a_subset(self):
        # M1: blocks of two heads are out at the same time; a short wait on one block's drives returns at its
        # time-out with that block still missing, the other block's answer is collected independently.
        self.boards.send(dict(node=612, drive=3, cmd=73, params=[300]))
        self.boards.send(dict(node=612, drive=1, cmd=73, params=[100]))
        start = time.monotonic()
        out = self.boards.wait(dict(targets=[dict(node=612, drive=3, codes=[73])], timeout=0.02))
        self.assertLess(time.monotonic() - start, 0.1)
        self.assertEqual(out["missing"], [dict(node=612, drive=3)])
        out = self.boards.wait(dict(targets=[dict(node=612, drive=1, codes=[73])], timeout=3))
        self.assertEqual([r["position"] for r in out["results"]], [100])
        out = self.boards.wait(dict(targets=[dict(node=612, drive=3, codes=[73])], timeout=3))
        self.assertEqual([r["position"] for r in out["results"]], [300])

    def test_stop_ends_a_running_wait(self):
        # Carry-over: a wait for a drive that never answers ends within 0,5 s after 'stop', not at its time-out.
        self.boards.send(dict(node=613, drive=1, cmd=73, params=[100]))  # no board 613: never answers
        done = {}
        t = threading.Thread(target=lambda: done.update(out=self.boards.wait(dict(targets=[dict(node=613, drive=1, codes=[73])], timeout=30))))
        start = time.monotonic()
        t.start()
        time.sleep(0.2)
        self.boards.stop()
        t.join(2)
        self.assertFalse(t.is_alive())
        self.assertLess(time.monotonic() - start, 0.8)
        self.assertTrue(done["out"]["stopped"])
        self.assertEqual(done["out"]["missing"], [dict(node=613, drive=1)])


TRACE_A = [  # board 626, DISPENSE drive 4: update, update, done (2), then an error-flagged zero set (rsp FF on 0x81)
    "00 08 04 AA 00 00 00 00", "00 07 04 AA 00 00 00 00", "00 06 04 AA 00 00 00 00", "00 05 04 AA 00 00 00 00",
    "00 04 04 AA 01 00 00 00", "00 03 04 AA 00 00 00 00", "00 02 04 AA 0F 00 00 00", "01 01 04 AA 00 00 00 00",
    "00 08 04 AA 00 00 00 00", "00 07 04 AA 00 00 00 00", "00 06 04 AA E0 36 00 00", "00 05 04 AA 00 00 00 00",
    "00 04 04 AA 01 00 00 00", "00 03 04 AA 00 00 00 00", "00 02 04 AA 12 00 00 00", "01 01 04 AA E0 36 00 00",
    "00 08 04 AA 00 00 00 00", "00 07 04 AA 00 00 00 00", "00 06 04 AA 00 00 00 00", "00 05 04 AA 0A 00 00 00",
    "00 04 04 AA 01 00 00 00", "00 03 04 AA 00 00 00 00", "00 02 04 AA 1A 00 00 00", "02 01 04 AA 00 D0 07 00",
]
ERROR_SET = ["00 88 04 AA 00 00 00 00", "00 87 04 AA 00 00 00 00", "00 86 04 AA 00 00 00 00", "00 85 04 AA 00 00 00 00",
             "00 84 04 AA 00 00 00 00", "00 83 04 AA 00 00 00 00", "00 82 04 AA 00 00 00 00", "FF 81 04 AA 00 00 00 00"]
DISPENSE_626_4 = dict(node=626, drive=4, cmd=0x49, params=[512000, 10, 0, 51200, 51200, 51200, 0, 0])


class TraceTests(unittest.TestCase):
    """Real frames from the machine (board 626), replayed on a virtual bus; no board simulation."""
    def setUp(self):
        self.channel = "vsd-trace-%d" % time.monotonic_ns()
        self.board = can.Bus(interface="virtual", channel=self.channel)
        self.tmp = tempfile.TemporaryDirectory()
        self.boards = Boards(Path(self.tmp.name), bus_factory=lambda a: can.Bus(interface="virtual", channel=self.channel))
        self.boards.connect({})

    def tearDown(self):
        self.boards.disconnect()
        self.board.shutdown()
        self.tmp.cleanup()

    def play(self, frames, node=626):
        for f in frames:
            self.board.send(can.Message(arbitration_id=node, is_extended_id=False, data=bytes.fromhex(f.replace(" ", ""))))
        time.sleep(0.15)  # let the receiver thread take them

    def wait(self, timeout, codes=(73, 2)):
        return self.boards.wait(dict(targets=[dict(node=626, drive=4, codes=list(codes))], timeout=timeout))

    def test_done_then_error_set_is_success_whatever_the_poll_timing(self):
        # All frames (incl. the trailing error set) arrive before the first poll.
        self.boards.send(DISPENSE_626_4)
        self.play(TRACE_A + ERROR_SET)
        out = self.wait(0.05)
        self.assertEqual([(r["code"], r["error"], r["position"]) for r in out["results"]], [(2, False, 512000)])
        self.assertEqual(sorted(self.boards.err_regs[(626, 4)]), list(range(1, 9)), "error set kept apart")
        self.assertEqual(self.boards.regs[(626, 4)][5], 10, "normal registers not overwritten by the error set")

    def test_sliced_polls_before_between_and_after(self):
        self.boards.send(DISPENSE_626_4)
        self.assertEqual(self.wait(0.02)["missing"], [dict(node=626, drive=4)])      # before any frame
        self.play(TRACE_A[:16])                                                        # two updates
        self.assertEqual(self.wait(0.02)["missing"], [dict(node=626, drive=4)])      # between: still running
        self.play(TRACE_A[16:] + ERROR_SET)                                            # done + error set
        out = self.wait(0.02)                                                          # after
        self.assertEqual([(r["code"], r["error"], r["position"]) for r in out["results"]], [(2, False, 512000)])

    def test_wait_running_while_frames_arrive(self):
        self.boards.send(DISPENSE_626_4)
        threading.Timer(0.1, lambda: self.play(TRACE_A + ERROR_SET)).start()
        out = self.wait(3)
        self.assertEqual([(r["code"], r["error"]) for r in out["results"]], [(2, False)])

    def test_error_set_before_any_accepted_reply_fails(self):
        self.boards.send(DISPENSE_626_4)
        self.play(TRACE_A[:8] + ERROR_SET)
        out = self.wait(0.05)
        self.assertEqual([(r["code"], r["error"]) for r in out["results"]], [(0xFFFF, True)])

    def test_status_answered_by_update(self):
        # Trace B: STATUS (0x47) to drive 1 answered with rsp 1 (update) carrying the position in p1.
        self.boards.send(dict(node=626, drive=1, cmd=0x47, params=[]))
        self.play(["00 08 01 AA 00 00 00 00", "00 07 01 AA 00 00 00 00", "00 06 01 AA 00 00 00 00", "00 05 01 AA 00 00 00 00",
                   "00 04 01 AA 00 00 00 00", "00 03 01 AA 00 00 00 00", "00 02 01 AA 00 00 00 00", "01 01 01 AA 00 64 00 00"])
        out = self.boards.wait(dict(targets=[dict(node=626, drive=1, codes=[71, 1, 72])], timeout=0.5))
        self.assertEqual([(r["code"], r["position"]) for r in out["results"]], [(1, 25600)])
        # Without UPDATE in the list it would never complete (the old behaviour).
        out = self.boards.wait(dict(targets=[dict(node=626, drive=1, codes=[71])], timeout=0.05))
        self.assertEqual(out["missing"], [dict(node=626, drive=1)])


class WireTests(unittest.TestCase):
    """K1: the step target p1 reaches the bus exactly as signed 32-bit little endian (rec 1 frame)."""
    def test_p1_steps_exact_on_the_wire(self):
        channel = "vsd-wire-%d" % time.monotonic_ns()
        listener = can.Bus(interface="virtual", channel=channel)
        tmp = tempfile.TemporaryDirectory()
        boards = Boards(Path(tmp.name), bus_factory=lambda a: can.Bus(interface="virtual", channel=channel))
        boards.connect({})
        try:
            for p1 in (1, 51199, 51200, 51201, 25600, -12800, -11800, -64000, 2147483647, -2147483648):
                # Camelot split: whole revolutions and rest truncated toward zero (rest has the sign of p1).
                p2 = -(-p1 // 51200) if p1 < 0 else p1 // 51200
                p3 = p1 - p2 * 51200
                boards.send(dict(node=622, drive=1, cmd=74, params=[p1, p2, p3, 51200, 51200, 51200, 0, 0]))
                frames = []
                while len(frames) < 8:
                    msg = listener.recv(timeout=1)
                    self.assertIsNotNone(msg)
                    frames.append(bytes(msg.data))
                cmd = frames[-1]
                self.assertEqual((cmd[0], cmd[1], cmd[2], cmd[3]), (74, 1, 1, 0xBB))
                self.assertEqual(int.from_bytes(cmd[4:8], "little", signed=True), p1)
                rec3 = [f for f in frames if f[1] == 3][0]
                self.assertEqual(int.from_bytes(rec3[4:8], "little", signed=True), p3)
                rec2 = [f for f in frames if f[1] == 2][0]
                self.assertEqual(int.from_bytes(rec2[4:8], "little", signed=True), p2)
            with self.assertRaises(OverflowError):
                boards.send(dict(node=622, drive=1, cmd=74, params=[2147483648]))
        finally:
            boards.disconnect(); listener.shutdown(); tmp.cleanup()


class ScanTests(unittest.TestCase):
    def test_scan_reports_heartbeat_nodes(self):
        channel = "vsd-scan-%d" % time.monotonic_ns()
        sender = can.Bus(interface="virtual", channel=channel)
        tmp = tempfile.TemporaryDirectory()
        boards = Boards(Path(tmp.name), bus_factory=lambda a: can.Bus(interface="virtual", channel=channel))
        stop = threading.Event()
        def beat():
            while not stop.is_set():
                sender.send(can.Message(arbitration_id=622, is_extended_id=False, data=bytes([0xFF, 0xFF, 0, 0xAA, 1, 0, 0, 0])))
                time.sleep(0.1)
        t = threading.Thread(target=beat, daemon=True)
        t.start()
        out = boards.scan(dict(seconds=0.6, channels=[0]))
        stop.set(); t.join(1); sender.shutdown(); tmp.cleanup()
        row = out["channels"][0]
        self.assertEqual(row["heartbeat"], [622])
        self.assertGreater(row["frames"], 2)


if __name__ == "__main__":
    unittest.main()
