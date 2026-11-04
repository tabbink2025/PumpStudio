"""Normalize measurement records for display without changing hardware state."""
import math


def number(value):
    return value if isinstance(value,(int,float)) and not isinstance(value,bool) and math.isfinite(value) else None


def normalize(kind,row):
    if kind=='Dosering':
        target=number(row.get('target_g',row.get('requested_g')))
        mass=number(row.get('mass_g',row.get('dispensed_g')))
    else:
        target=None
        mass=number(row.get('mass_g'))
    # A missing measurement is not a zero dose. Compute error only from data.
    error=mass-target if target is not None and mass is not None else None
    pct=100*error/target if error is not None and target else None
    index=str(row.get('repeat',row.get('i','—')))
    if kind=='360°':index+=' · '+str(row.get('angle_deg','—'))+'°'
    return dict(kind=kind,index=index,target_g=target,mass_g=mass,error_g=error,error_pct=pct,
                correction_pct=number(row.get('manual_correction_pct')),
                curve_target_g=number(row.get('curve_target_g')),
                cumulative_g=number(row.get('cumulative_g')),
                start_steps=row.get('start_position_steps',row.get('start_steps')),
                end_steps=row.get('end_position_steps',row.get('position_steps',row.get('target_steps') if row.get('status')=='completed' else None)),
                status=row.get('status','unknown'),error=str(row.get('error','')))


def records_from_payload(payload):
    if not isinstance(payload,dict) or not isinstance(payload.get('records'),list):
        raise ValueError('Dit bestand bevat geen meetresultaten')
    rows=payload['records']
    if any(not isinstance(row,dict) for row in rows):raise ValueError('Ongeldige meetregels')
    for row in rows:
        kind='360°' if 'angle_deg' in row else ('Dosering' if number(row.get('target_g',row.get('requested_g'))) is not None else 'Kalibratie')
        yield normalize(kind,row)
