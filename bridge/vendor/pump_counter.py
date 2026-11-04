"""Command-coordinate counter; never presented as measured shaft position."""
import time
from ceramic_pump import integer, move_timeout
from pump_debug_characterization import CharacterizationAdapter


def position_binding(config):
    return dict(node=config.node,motor=config.motor,steps_rev=config.steps_rev)


class CommandCounterAdapter(CharacterizationAdapter):
    def __init__(self, app, config, position_tolerance=64):
        super().__init__(app,config,position_tolerance)
        self.require_feedback=bool(app.wait_done_var.get())

    def establish_zero(self):
        """Explicit operator action: choose current position as new origin."""
        if self.require_feedback:
            super().set_relative_reference()
        else:
            from pump_debug_legacy import COMMANDS
            self.check()
            if not self.app._sequence(self.cfg.node,self.cfg.motor,COMMANDS['ZERO'],0,0,0,self.cfg.decel):
                raise RuntimeError('Teller-nul kon niet worden verzonden')
            self.sleep(.25)
            self.app.commanded[self.cfg.motor]=0
            self.reference_valid=True
            self.homed=False
            self.homing_status='software_origin_ZERO_sent_unverified'
        self.reference_mode='relative'
        self.app.counter_valid=True
        self.app.counter_bus=self.app.bus
        self.app.counter_context=position_binding(self.cfg)

    def set_relative_reference(self):
        """Curve starts at current counter; no HOME/ZERO or other movement."""
        self.check()
        if (not self.app.counter_valid or self.app.counter_bus is not self.app.bus
                or self.app.counter_context!=position_binding(self.cfg)):
            raise RuntimeError('Kies eerst startpositie; verbinding of motorinstelling gewijzigd')
        self.reference_mode='relative'
        self.reference_valid=True
        self.homed=False
        self.homing_status='continuous_sent_position_counter'

    def check_reference(self):
        super().check_reference()
        if (not self.app.counter_valid or self.app.counter_bus is not self.app.bus
                or self.app.counter_context!=position_binding(self.cfg)):
            raise RuntimeError('Software-referentie ongeldig; startpositie opnieuw kiezen')

    def move_absolute(self,target):
        try:
            self.check_reference()
            target=integer(target,'Absolute doelpositie')
            start=self.position
            if target<=start:
                raise ValueError('Alleen vooruit bewegen')
            if self.require_feedback:
                super().move_absolute(target)
            else:
                from pump_debug_legacy import COMMANDS
                since=time.monotonic()
                self.last_motion_feedback=None
                self.local.curve_motion=True
                try:
                    if not self.app._sequence(self.cfg.node,self.cfg.motor,COMMANDS['MOVE_TO'],
                                              target,self.cfg.speed,self.cfg.accel,self.cfg.decel):
                        raise RuntimeError('Bewegingsopdracht niet volledig verzonden')
                    # Set absolute target, equivalent to adding target-start.
                    # Do not add absolute targets to one another.
                    self.app.commanded[self.cfg.motor]=target
                    self.sleep(move_timeout(target-start,self.cfg.speed,self.cfg.accel,self.cfg.decel,.5))
                    response=self.responses.get((74,1))
                    reported=response[1] if response and response[0]>=since else None
                    self.last_motion_feedback=dict(target_position_steps=target,
                        reported_position_steps=reported,
                        controller_position_error_steps=None if reported is None else reported-target,
                        sent_delta_steps=target-start,counter_steps=target,
                        position_basis='sent_absolute_targets',physical_position_verified=False,
                        motion_acknowledged=reported is not None,
                        position_tolerance_steps=self.position_tolerance)
                    self.app._event('COUNTER',None,b'',str(self.last_motion_feedback))
                finally:
                    self.local.curve_motion=False
            self.app.counter_valid=True
        except Exception:
            self.reference_valid=False
            self.app.counter_valid=False
            raise

    def abort(self):
        self.app.counter_valid=False
        super().abort()
