//! 每 hart 用户（U）/维护（M）两类别有界时间轮转器。
//!
//! 有符号余额：实际执行全额扣减；轮到某类时补其 q，余额 > 0 才执行，
//! 余额 ≤ 0 时跳过（仲裁推进，不是无工作）。负债保留，下界为 −δ；
//! 空闲类不积攒正信用。trap/Resume 不重发额度。

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    User,
    Maintenance,
}

impl Class {
    const fn other(self) -> Self {
        match self {
            Class::User => Class::Maintenance,
            Class::Maintenance => Class::User,
        }
    }
    const fn index(self) -> usize {
        match self {
            Class::User => 0,
            Class::Maintenance => 1,
        }
    }
}

/// 一次仲裁的结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// 执行该类，至多 `budget` tick 后回到检查边界。
    Run { class: Class, budget: u64 },
    /// 双方有工作但所选类余额仍非正：仲裁已推进，服务 pending 后再仲裁，不得 WFI。
    Advance,
    /// 两类均无可运行工作，可 idle。
    Idle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rotor {
    quantum: [u64; 2],
    balance: [i64; 2],
    current: Class,
}

impl Rotor {
    pub fn new(user_quantum: u64, maintenance_quantum: u64) -> Self {
        assert!(user_quantum > 0 && maintenance_quantum > 0, "rotor quanta must be nonzero");
        Self {
            quantum: [user_quantum, maintenance_quantum],
            balance: [to_i64(user_quantum), 0],
            current: Class::User,
        }
    }

    pub const fn balance(&self, class: Class) -> i64 {
        self.balance[class.index()]
    }

    /// 在安全点仲裁。`user_ready`/`maint_ready` 为本 hart 当前可领取事实。
    pub fn choose(&mut self, user_ready: bool, maint_ready: bool) -> Choice {
        let ready = |c: Class| match c {
            Class::User => user_ready,
            Class::Maintenance => maint_ready,
        };
        // 没有工作的类：不积攒正信用，保留负债。
        for c in [Class::User, Class::Maintenance] {
            if !ready(c) && self.balance[c.index()] > 0 {
                self.balance[c.index()] = 0;
            }
        }
        match (user_ready, maint_ready) {
            (false, false) => return Choice::Idle,
            (true, false) | (false, true) => {
                // 独占：唯一可运行类借用全部能力，负债仍按规则扣减并补足。
                let c = if user_ready { Class::User } else { Class::Maintenance };
                self.current = c;
                if self.balance[c.index()] <= 0 {
                    self.refill(c);
                }
                return self.run_or_advance(c);
            }
            (true, true) => {}
        }
        let c = self.current;
        if self.balance[c.index()] > 0 {
            return self.run_or_advance(c);
        }
        // 当前类耗尽：转给另一类并补一次量子（每次仲裁至多一步）。
        let next = c.other();
        self.current = next;
        self.refill(next);
        self.run_or_advance(next)
    }

    /// 回到检查边界后按实际占用全额扣账。
    pub fn charge(&mut self, class: Class, actual: u64) {
        let b = &mut self.balance[class.index()];
        *b = b.saturating_sub(to_i64(actual));
    }

    fn refill(&mut self, c: Class) {
        let b = &mut self.balance[c.index()];
        *b = b.saturating_add(to_i64(self.quantum[c.index()]));
    }

    fn run_or_advance(&self, c: Class) -> Choice {
        let b = self.balance[c.index()];
        if b > 0 {
            Choice::Run { class: c, budget: b as u64 }
        } else {
            Choice::Advance
        }
    }
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).expect("tick value exceeds rotor range")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 模拟持续竞争：每次运行实际消耗 = budget + overrun(class)。
    fn simulate(qu: u64, qm: u64, over_u: u64, over_m: u64, rounds: usize) -> (u64, u64) {
        let mut r = Rotor::new(qu, qm);
        let (mut su, mut sm) = (0u64, 0u64);
        for _ in 0..rounds {
            match r.choose(true, true) {
                Choice::Run { class, budget } => {
                    let actual = budget
                        + match class {
                            Class::User => over_u,
                            Class::Maintenance => over_m,
                        };
                    r.charge(class, actual);
                    match class {
                        Class::User => su += actual,
                        Class::Maintenance => sm += actual,
                    }
                }
                Choice::Advance => {}
                Choice::Idle => panic!("both classes have work"),
            }
        }
        (su, sm)
    }

    #[test]
    fn overrun_debt_is_carried_not_reset() {
        // DeepAsh 反例：q=1:1，M 每次超出 1。重置余额会长期 2:1。
        let (su, sm) = simulate(1, 1, 0, 1, 10_000);
        let drift = su.abs_diff(sm);
        assert!(drift <= 4, "share drift {su} vs {sm}");
    }

    #[test]
    fn weighted_share_holds() {
        let (su, sm) = simulate(3, 1, 0, 0, 10_000);
        assert!((su as i64 - 3 * sm as i64).abs() <= 6, "{su} vs {sm}");
    }

    #[test]
    fn debt_lower_bound_is_one_overrun() {
        let mut r = Rotor::new(5, 5);
        for _ in 0..1000 {
            if let Choice::Run { class, budget } = r.choose(true, true) {
                r.charge(class, budget + 2);
                assert!(r.balance(class) >= -2);
            }
        }
    }

    #[test]
    fn nonpositive_balance_advances_not_idles() {
        let mut r = Rotor::new(1, 1);
        // 让两类都欠下大于一个量子的债。
        if let Choice::Run { class, .. } = r.choose(true, true) {
            r.charge(class, 5);
        }
        if let Choice::Run { class, .. } = r.choose(true, true) {
            r.charge(class, 5);
        }
        let mut advances = 0;
        loop {
            match r.choose(true, true) {
                Choice::Advance => advances += 1,
                Choice::Run { .. } => break,
                Choice::Idle => panic!("must not idle with ready work"),
            }
        }
        assert!(advances > 0);
    }

    #[test]
    fn idle_class_accumulates_no_credit() {
        let mut r = Rotor::new(4, 4);
        for _ in 0..10 {
            if let Choice::Run { class, budget } = r.choose(true, false) {
                assert_eq!(class, Class::User);
                r.charge(class, budget);
            }
        }
        assert!(r.balance(Class::Maintenance) <= 0);
        assert_eq!(r.choose(false, false), Choice::Idle);
    }

    #[test]
    fn exclusive_class_borrows_all_capacity() {
        let mut r = Rotor::new(1, 1);
        for _ in 0..10 {
            match r.choose(false, true) {
                Choice::Run { class, budget } => {
                    assert_eq!(class, Class::Maintenance);
                    r.charge(class, budget);
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}
