//! 每 hart 用户（U）/维护（M）两类别费用与有限服务机会轮转器。
//!
//! 有符号余额：采样费用全额扣减；每轮 q tick 信用及 q 次机会，任一耗尽
//! 即让出。负债保留，未用正信用不积攒；trap/Resume 不重发额度。

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
    balance: [i128; 2],
    opportunities: [u64; 2],
    current: Class,
}

impl Rotor {
    pub fn new(user_quantum: u64, maintenance_quantum: u64) -> Self {
        assert!(
            user_quantum > 0 && maintenance_quantum > 0,
            "rotor quanta must be nonzero"
        );
        Self {
            quantum: [user_quantum, maintenance_quantum],
            balance: [i128::from(user_quantum), 0],
            opportunities: [user_quantum, 0],
            current: Class::User,
        }
    }

    pub const fn balance(&self, class: Class) -> i128 {
        self.balance[class.index()]
    }

    pub const fn exhausted(&self, class: Class) -> bool {
        self.balance[class.index()] <= 0 || self.opportunities[class.index()] == 0
    }

    /// 同一类别的下一返回/执行机会；只领取，不补量子。
    pub fn resume(&mut self, class: Class) -> bool {
        assert_eq!(self.current, class, "resume outside selected class");
        if self.exhausted(class) {
            return false;
        }
        self.opportunities[class.index()] -= 1;
        true
    }

    /// 在安全点仲裁。`user_ready`/`maint_ready` 为本 hart 当前可领取事实。
    pub fn choose(&mut self, user_ready: bool, maint_ready: bool) -> Choice {
        let ready = |c: Class| match c {
            Class::User => user_ready,
            Class::Maintenance => maint_ready,
        };
        // 没有工作的类：不积攒正信用，保留负债。
        for c in [Class::User, Class::Maintenance] {
            if !ready(c) {
                self.balance[c.index()] = self.balance[c.index()].min(0);
                self.opportunities[c.index()] = 0;
            }
        }
        match (user_ready, maint_ready) {
            (false, false) => return Choice::Idle,
            (true, false) | (false, true) => {
                // 独占：唯一可运行类借用全部能力，负债仍按规则扣减并补足。
                let c = if user_ready {
                    Class::User
                } else {
                    Class::Maintenance
                };
                self.current = c;
                if self.exhausted(c) {
                    self.refill(c);
                }
                return self.run_or_advance(c);
            }
            (true, true) => {}
        }
        let c = self.current;
        if !self.exhausted(c) {
            return self.run_or_advance(c);
        }
        // 机会耗尽也结束本轮；正余额不跨轮积攒，旧负债仍在。
        self.balance[c.index()] = self.balance[c.index()].min(0);
        self.opportunities[c.index()] = 0;
        let next = c.other();
        self.current = next;
        self.refill(next);
        self.run_or_advance(next)
    }

    /// 回到检查边界后按实际占用全额扣账。
    pub fn charge(&mut self, class: Class, actual: u64) {
        let b = &mut self.balance[class.index()];
        *b = b
            .checked_sub(i128::from(actual))
            .expect("rotor cost exceeds epoch range");
    }

    fn refill(&mut self, c: Class) {
        self.opportunities[c.index()] = self.quantum[c.index()];
        let b = &mut self.balance[c.index()];
        *b = (*b)
            .min(0)
            .checked_add(i128::from(self.quantum[c.index()]))
            .expect("rotor credit exceeds epoch range");
    }

    fn run_or_advance(&mut self, c: Class) -> Choice {
        let b = self.balance[c.index()];
        if !self.exhausted(c) {
            self.opportunities[c.index()] -= 1;
            Choice::Run {
                class: c,
                budget: b as u64,
            }
        } else {
            Choice::Advance
        }
    }
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

    #[test]
    fn zero_cost_classes_alternate_after_their_finite_service_opportunities() {
        let mut r = Rotor::new(2, 3);
        for _ in 0..3 {
            for (class, count) in [(Class::User, 2), (Class::Maintenance, 3)] {
                for _ in 0..count {
                    assert_eq!(
                        r.choose(true, true),
                        Choice::Run {
                            class,
                            budget: count,
                        }
                    );
                    r.charge(class, 0);
                }
                assert_eq!(r.balance(class), i128::from(count));
                assert!(r.exhausted(class));
            }
        }
    }

    #[test]
    fn resume_consumes_same_round_opportunity_not_each_accounting_fragment() {
        let mut r = Rotor::new(2, 1);
        assert_eq!(
            r.choose(true, true),
            Choice::Run {
                class: Class::User,
                budget: 2,
            }
        );
        r.charge(Class::User, 0);
        r.charge(Class::User, 0);
        assert!(r.resume(Class::User));
        // 最后一份已经授予，balance 仍可装填本次 timer；只拒绝下一份。
        assert_eq!(r.balance(Class::User), 2);
        r.charge(Class::User, 0);
        assert!(!r.resume(Class::User));
        assert_eq!(
            r.choose(true, true),
            Choice::Run {
                class: Class::Maintenance,
                budget: 1,
            }
        );
    }

    #[test]
    fn unsuccessful_zero_cost_selections_cannot_keep_the_other_class_waiting() {
        let mut r = Rotor::new(2, 1);
        for _ in 0..2 {
            let Choice::Run { class, .. } = r.choose(true, true) else {
                panic!("ready class lost service");
            };
            assert_eq!(class, Class::User);
            // 资格快照后的选择可以落败；仍结束该尝试的真实计费（此处为0）。
            r.charge(class, 0);
        }
        assert!(matches!(
            r.choose(true, true),
            Choice::Run {
                class: Class::Maintenance,
                ..
            }
        ));
    }

    #[test]
    fn zero_cost_exclusive_class_borrows_without_banking_credit() {
        let mut r = Rotor::new(2, 1);
        for _ in 0..8 {
            assert!(matches!(
                r.choose(true, false),
                Choice::Run {
                    class: Class::User,
                    budget: 2,
                }
            ));
            r.charge(Class::User, 0);
        }
        assert_eq!(r.balance(Class::User), 2);
        assert!(matches!(
            r.choose(true, true),
            Choice::Run {
                class: Class::Maintenance,
                ..
            }
        ));
        assert_eq!(r.balance(Class::User), 0);
    }
}
