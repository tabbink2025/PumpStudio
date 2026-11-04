"""Gravimetric outflow characterization; never infers internal piston motion."""
import csv
import json
import math
import statistics
import time
from datetime import datetime
from pathlib import Path
from uuid import uuid4

from ceramic_pump import Measurements, integer, positive


def angle_targets(steps_rev, interval):
    steps_rev = integer(steps_rev, "Stappen/omwenteling")
    interval = positive(interval, "Hoekinterval")
    if interval > 360 or math.ceil(360 / interval) > min(10000, steps_rev):
        raise ValueError("Hoekinterval buiten bereik of te klein")
    angles = [min(i * interval, 360.0) for i in range(1, math.ceil(360 / interval) + 1)]
    targets = [round(steps_rev * angle / 360) for angle in angles]
    if any(b <= a for a, b in zip([0] + targets, targets)):
        raise ValueError("Hoekinterval geeft dubbele motorposities")
    return list(zip(angles, targets))


def step_targets(steps_rev, interval_steps):
    steps_rev = integer(steps_rev, "Stappen/omwenteling")
    interval_steps = integer(interval_steps, "Meetinterval in stappen", steps_rev)
    offsets = list(range(interval_steps, steps_rev, interval_steps)) + [steps_rev]
    if len(offsets) > 10000:
        raise ValueError("Te veel meetpunten per omwenteling")
    return [(offset * 360 / steps_rev, offset) for offset in offsets]


class Characterization(Measurements):
    def guard(self):
        getattr(self.pump, "check_reference", self.pump.check_homed)()

    def stable_weight(self, hold=None, timeout=None):
        self.guard()
        result = super().stable_weight(hold, timeout)
        self.guard()
        return result

    def sample(self):
        self.guard()
        return super().sample()

    def run(self, interval=10, repeats=5, settle=1.0, home_timeout=60,
            reference_mode="home", interval_steps=None):
        if reference_mode not in ("home", "relative"):
            raise ValueError("Onbekende referentiemodus")
        targets = (step_targets(self.pump.cfg.steps_rev, interval_steps) if interval_steps is not None
                   else angle_targets(self.pump.cfg.steps_rev, interval))
        repeats = integer(repeats, "Herhalingen", 100)
        settle = float(settle)
        home_timeout = positive(home_timeout, "Homingtimeout")
        if not math.isfinite(settle) or settle < 0 or settle > 300:
            raise ValueError("Uitlooptijd moet tussen 0 en 300 s liggen")
        if repeats * len(targets) > 100000 or repeats * self.pump.cfg.steps_rev > 2147483647:
            raise ValueError("Meetreeks te groot")
        self.directory.mkdir(parents=True, exist_ok=True)
        self.base = self.directory / ("characterization_" + datetime.now().strftime("%Y%m%d_%H%M%S_") + uuid4().hex[:8])
        self.payload = dict(schema_version=2, test="360° Characterization", status="running",
                            started=datetime.now().astimezone().isoformat(),
                            reference_mode=reference_mode, physical_home_known=reference_mode == "home",
                            physical_position_verified=False, angle_basis="commanded_motor_steps",
                            interval_steps=interval_steps,
                            interval_deg=(int(interval_steps)*360/self.pump.cfg.steps_rev if interval_steps is not None else float(interval)),
                            repeats=repeats, settle_s=settle, home_timeout_s=home_timeout,
                            context=self.pump.cfg.context(), metadata=getattr(self, "test_metadata", {}), records=self.records,
                            scale_samples=self.scale_samples, homing_events=[],
                            interpretation="Gemeten uitstroom; geen directe interne plunjerverplaatsing.",
                            homing_monitor="HOME_DONE + ZERO; verlies via bestaande home_done-status, fouten of stop. Geen continue fysieke hoekmeting.")
        try:
            self.save()
            if reference_mode == "home":
                self.pump.home(home_timeout)
            else:
                self.pump.set_relative_reference()
                self.payload['homing_monitor'] = "Relatieve software-nul met bevestigde ZERO; geen fysieke home. Geldig tot stop, fout of onzekere beweging."
            self.payload['reference_origin_steps'] = self.pump.position
            self.payload['homing_events'].append(dict(time=datetime.now().astimezone().isoformat(), status=self.pump.homing_status, position=self.pump.position))
            self.guard()
            for repeat in range(1, repeats + 1):
                self.guard()
                with self.scale_lock:
                    if not self.scale.tare():
                        raise RuntimeError("Tarreren mislukt")
                baseline = self.stable_weight()
                previous = baseline
                origin = self.pump.position
                self.records.append(dict(repeat=repeat, angle_deg=0, actual_angle_deg=0,
                                         target_steps=origin, position_steps=origin, mass_g=0., cumulative_g=0.,
                                         weight_g=baseline, homed=self.pump.homed, reference_mode=reference_mode,
                                         reference_valid=True, relative_position_steps=origin-self.payload['reference_origin_steps'],
                                         phase_steps=0, scale_status="stable", status="completed"))
                previous_offset = 0
                for angle, offset in targets:
                    self.guard()
                    row = dict(repeat=repeat, angle_deg=angle, actual_angle_deg=offset * 360 / self.pump.cfg.steps_rev,
                               target_steps=origin + offset, start_position_steps=self.pump.position,
                               reference_mode=reference_mode, interval_steps=offset-previous_offset,
                               relative_position_steps=origin+offset-self.payload['reference_origin_steps'],
                               phase_steps=offset % self.pump.cfg.steps_rev,
                               time=datetime.now().astimezone().isoformat(), status="running", scale_status="pending")
                    self.records.append(row)
                    try:
                        self.pump.move_absolute(origin + offset)
                        self.pump.sleep(settle)
                        weight = self.stable_weight()
                        row.update(weight_g=weight, mass_g=weight-previous, cumulative_g=weight-baseline,
                                   scale_status="stable", status="completed")
                        previous = weight
                        previous_offset = offset
                    except Exception as exc:
                        row.update(status="failed", error=str(exc), scale_status="unconfirmed")
                        raise
                    finally:
                        row.update(position_steps=self.pump.position, homed=self.pump.homed,
                                   homing_status=self.pump.homing_status,
                                   reference_valid=getattr(self.pump, 'reference_valid', self.pump.homed))
                        if getattr(self.pump, 'last_motion_feedback', None):
                            row.update(self.pump.last_motion_feedback)
                        self.save()
                        self.result_sink(dict(row))
            self.guard()
            self.payload['status'] = 'completed'
        except Exception as exc:
            self.payload.update(status="cancelled" if self.pump.cancel.is_set() else "failed", error=str(exc))
            try:
                self.pump.abort()
            except Exception as stop_error:
                self.payload['stop_error'] = str(stop_error)
            raise
        finally:
            self.payload.update(ended=datetime.now().astimezone().isoformat(),
                                final_homed=self.pump.homed, final_reference_valid=getattr(self.pump, 'reference_valid', self.pump.homed), final_position_steps=self.pump.position,
                                final_homing_status=self.pump.homing_status)
            self.save()
        return self.payload

    def summary(self):
        groups = {}
        for row in self.records:
            if row['status'] == 'completed':
                groups.setdefault(row['angle_deg'], []).append(row)
        result = []
        for angle, rows in sorted(groups.items()):
            group = dict(angle_deg=angle, n=len(rows))
            for key in ('mass_g', 'cumulative_g'):
                values = [r[key] for r in rows]
                group[key] = statistics.mean(values)
                group[key + '_sd'] = statistics.stdev(values) if len(values) > 1 else 0.
            result.append(group)
        return result

    def save(self):
        self.payload['summary'] = self.summary()
        path = self.base.with_suffix('.json')
        temp = path.with_suffix('.tmp')
        temp.write_text(json.dumps(self.payload, indent=2, ensure_ascii=False), encoding='utf-8')
        for attempt in range(6):
            try:
                temp.replace(path)
                break
            except PermissionError:
                if attempt == 5:
                    raise
                time.sleep(.02 * (attempt + 1))
        for suffix, rows in (('_metadata', [{key: json.dumps(value, ensure_ascii=False) for key, value in self.payload.items() if key not in ('records', 'scale_samples', 'summary')}]), ('', self.records), ('_scale', self.scale_samples), ('_summary', self.payload['summary'])):
            keys = sorted({key for row in rows for key in row})
            with self.base.with_name(self.base.name + suffix + '.csv').open('w', newline='', encoding='utf-8-sig') as stream:
                writer = csv.DictWriter(stream, fieldnames=keys)
                writer.writeheader()
                writer.writerows(rows)


def plot_characterization(payload):
    from matplotlib.figure import Figure
    figure = Figure(figsize=(9, 6), tight_layout=True)
    for index, (key, title) in enumerate((('cumulative_g', 'Cumulatieve uitstroom'), ('mass_g', 'Massa per hoekinterval'))):
        axis = figure.add_subplot(2, 1, index + 1)
        for repeat in range(1, payload['repeats'] + 1):
            rows = [r for r in payload['records'] if r['repeat'] == repeat and r['status'] == 'completed' and (key != 'mass_g' or r['angle_deg'] > 0)]
            axis.plot([r['angle_deg'] for r in rows], [r[key] for r in rows], alpha=.3)
        rows = [r for r in payload['summary'] if key != 'mass_g' or r['angle_deg'] > 0]
        axis.errorbar([r['angle_deg'] for r in rows], [r[key] for r in rows],
                      yerr=[r[key + '_sd'] for r in rows], capsize=3, color='black', label='Gemiddelde ± 1 SD')
        axis.set(title=title, xlabel=('Aangevraagde hoek vanaf relatieve start (°)' if payload.get('reference_mode') == 'relative' else 'Aangevraagde hoek vanaf homesensor (°)'), ylabel='Massa (g)')
        if key == 'cumulative_g' and payload.get('dose_curve'):
            curve = payload['dose_curve']
            axis.plot([s*360/curve['steps_rev'] for s in curve['steps']], curve['cumulative_g'],
                      linestyle='--', label='Monotone doseercurve')
        axis.legend()
        axis.grid(alpha=.2)
    figure.suptitle('Gemeten uitstroom; geen directe interne plunjerverplaatsing')
    return figure
