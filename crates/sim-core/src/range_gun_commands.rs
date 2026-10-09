use super::battle::commands::{Command, GunCommand, Outcome};
use super::BlastRange;

impl BlastRange {
    pub fn control(&mut self) {
        self.execute_command(Command::Gun(GunCommand::Control));
    }
    pub fn order_move(&mut self, destination: [i32; 2]) -> bool {
        self.execute_command(Command::Gun(GunCommand::Move { destination }))
            .accepted()
    }
    pub fn order_attack(&mut self, target: usize) -> bool {
        self.execute_command(Command::Gun(GunCommand::Attack { target }))
            .accepted()
    }
    pub fn order_stop(&mut self) {
        self.execute_command(Command::Gun(GunCommand::Stop));
    }
    pub fn set_auto_attack(&mut self, enabled: bool) {
        self.execute_command(Command::Gun(GunCommand::AutoAttack { enabled }));
    }
    /// A busy shot still cancels the previous target and updates the aim.
    /// Invalid coordinates are rejected before either operation.
    pub fn order_ground_fire(&mut self, at: [i32; 3]) -> bool {
        self.execute_command(Command::Gun(GunCommand::GroundFire { at }))
            .accepted()
    }
    pub fn refire(&mut self) -> bool {
        self.execute_command(Command::Gun(GunCommand::Refire))
            .accepted()
    }

    pub(super) fn apply_gun_command(&mut self, command: GunCommand) -> Outcome {
        match command {
            GunCommand::Control => {
                let changed = !self.player;
                self.control_inner();
                if changed {
                    Outcome::Applied
                } else {
                    Outcome::Unchanged
                }
            }
            GunCommand::Move { destination } => {
                if self.gun.kind.emplacement() {
                    Outcome::Rejected
                } else if self.order_move_inner(destination) {
                    Outcome::Applied
                } else {
                    // The old route is cleared and manual control is retained.
                    Outcome::PathFailed
                }
            }
            GunCommand::Attack { target } => {
                if self.order_attack_inner(target) {
                    Outcome::Applied
                } else {
                    Outcome::Rejected
                }
            }
            GunCommand::Stop => {
                self.order_stop_inner();
                Outcome::Applied
            }
            GunCommand::AutoAttack { enabled } => {
                self.set_auto_attack_inner(enabled);
                Outcome::Applied
            }
            GunCommand::GroundFire { at } => {
                if !self.valid_gun_aim(at) {
                    Outcome::Rejected
                } else if self.order_ground_fire_inner(at) {
                    Outcome::Applied
                } else {
                    Outcome::Busy
                }
            }
            GunCommand::Refire => {
                if self.refire_inner() {
                    Outcome::Applied
                } else {
                    Outcome::Busy
                }
            }
            GunCommand::Aim { at } => {
                if at.is_some_and(|p| !self.valid_gun_aim(p)) {
                    Outcome::Rejected
                } else if self.aim == at {
                    Outcome::Unchanged
                } else {
                    self.aim = at;
                    Outcome::Applied
                }
            }
        }
    }
    // Same bounds as the physical snapshot, including its off-map margin.
    fn valid_gun_aim(&self, at: [i32; 3]) -> bool {
        (0..3).all(|k| (-384..=self.size[k] as i32 + 384).contains(&at[k]))
    }
}
