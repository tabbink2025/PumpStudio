"""Phase-aware forward dosing from a measured periodic outflow curve."""
import bisect
import math
import statistics


def corrected_mass(grams, percent=0):
    """Manual feed-forward correction; negative reduces nominal curve dose."""
    grams=float(str(grams).replace(',','.'))
    percent=float(str(percent).replace(',','.'))
    if not math.isfinite(grams) or grams<=0:
        raise ValueError('Doelmassa moet positief en eindig zijn')
    if not math.isfinite(percent) or not -20<=percent<=20:
        raise ValueError('Handmatige correctie moet tussen -20 en +20% liggen')
    return grams*(1+percent/100)


class OutflowCurve:
    def __init__(self, payload, max_correction_g=0.002, allow_smoothing=False):
        if payload.get('status') != 'completed':
            raise ValueError('Alleen een volledig afgeronde meetreeks kan doseercurve worden')
        self.steps_rev = int(payload['context']['steps_rev'])
        groups = {}
        repeats = int(payload['repeats'])
        for row in payload['records']:
            if row['status'] != 'completed':
                raise ValueError('Meetreeks bevat onvoltooide meetpunten')
            offset = round(row['actual_angle_deg'] * self.steps_rev / 360)
            value = float(row['cumulative_g'])
            if not math.isfinite(value):
                raise ValueError('Ongeldige massameting')
            groups.setdefault(offset, {})[row['repeat']] = value
        self.steps = sorted(groups)
        if (not self.steps or self.steps[0] != 0 or self.steps[-1] != self.steps_rev
                or any(len(values) != repeats for values in groups.values())):
            raise ValueError('Onvolledige omwentelingen of ontbrekende herhalingen')
        raw = [statistics.mean(groups[s].values()) for s in self.steps]
        # Isotonic least-squares fit: small scale noise may not produce negative
        # outflow. Reject large adjustments rather than hiding a bad experiment.
        blocks = []
        for value in raw:
            blocks.append([max(0.0, value), 1])
            while len(blocks) > 1 and blocks[-2][0] > blocks[-1][0]:
                b, a = blocks.pop(), blocks.pop()
                n = a[1] + b[1]
                blocks.append([(a[0]*a[1] + b[0]*b[1])/n, n])
        fitted = [value for value, n in blocks for _ in range(n)]
        self.correction_g = max(abs(a-b) for a, b in zip(raw, fitted))
        self.smoothing_requested = bool(allow_smoothing)
        self.quality_warning = self.correction_g > max_correction_g
        if (self.quality_warning and not allow_smoothing) or fitted[0] != 0:
            raise ValueError('Uitstroomcurve te onregelmatig; controleer opvang en meet opnieuw')
        self.mass = fitted
        self.g_per_rev = fitted[-1]
        if self.g_per_rev <= max_correction_g:
            raise ValueError('Te weinig gemeten uitstroom voor doseercurve')

    def cumulative(self, position):
        turns, phase = divmod(position, self.steps_rev)
        index = bisect.bisect_right(self.steps, phase) - 1
        a, b = self.steps[index:index+2]
        value = self.mass[index] + (self.mass[index+1]-self.mass[index])*(phase-a)/(b-a)
        return turns*self.g_per_rev + value

    def target(self, start, grams):
        if not isinstance(start, int) or start < 0 or not math.isfinite(grams) or grams <= 0:
            raise ValueError('Positie en doelmassa moeten geldig en positief zijn')
        goal = self.cumulative(start) + grams
        lo = start + 1
        hi = start + (math.ceil(grams/self.g_per_rev)+1)*self.steps_rev
        hi = min(hi, 2147483647)
        if lo > hi or self.cumulative(hi) < goal:
            raise ValueError('Absolute positieteller heeft onvoldoende bereik')
        while lo < hi:
            middle = (lo+hi)//2
            if self.cumulative(middle) >= goal:
                hi = middle
            else:
                lo = middle+1
        return lo

    def context(self):
        return dict(steps_rev=self.steps_rev, steps=self.steps, cumulative_g=self.mass,
                    g_per_rev=self.g_per_rev, monotonic_correction_g=self.correction_g,
                    interpolation='piecewise_linear', trim=False,
                    smoothing_requested=self.smoothing_requested, quality_warning=self.quality_warning)
