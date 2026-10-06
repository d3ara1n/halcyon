//! 账户的采样 tick DRR 与有限服务机会。环身份由内核拥有；本模块拥有
//! 每轮信用/机会、在途预扣/结算和跨轮欠账，全部 grant 结清后才能封轮。

#[must_use = "a granted visit must be settled"]
#[derive(Debug, PartialEq, Eq)]
pub enum Visit {
    Grant(Grant),
    /// 无可预扣信用；可能仍等待原轮退款，不能因此补下一轮。
    Skip,
}

#[must_use = "a grant must be settled"]
#[derive(Debug, PartialEq, Eq)]
pub struct Grant {
    ticks: u64,
}
impl Grant {
    pub const fn ticks(&self) -> u64 {
        self.ticks
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ledger {
    deficit: i128,
    opportunities: u64,
    in_flight: u32,
    started: bool,
}
impl Default for Ledger {
    fn default() -> Self {
        Self::new()
    }
}
impl Ledger {
    pub const fn new() -> Self {
        Self {
            deficit: 0,
            opportunities: 0,
            in_flight: 0,
            started: false,
        }
    }
    pub const fn deficit(&self) -> i128 {
        self.deficit
    }
    pub const fn in_flight(&self) -> u32 {
        self.in_flight
    }
    pub const fn is_quiescent(&self) -> bool {
        self.in_flight == 0
    }
    pub const fn round_started(&self) -> bool {
        self.started
    }
    pub const fn round_complete(&self) -> bool {
        self.started && self.in_flight == 0 && (self.deficit <= 0 || self.opportunities == 0)
    }

    /// 使用者只在本轮首次访问时调用；所有旧 grant 必须已经结清。
    pub fn begin_round(&mut self, quantum: u64) {
        assert!(quantum != 0, "DRR quantum must be nonzero");
        assert!(
            !self.started && self.is_quiescent(),
            "DRR round already active"
        );
        self.deficit = self
            .deficit
            .checked_add(i128::from(quantum))
            .expect("DRR credit exceeds epoch range");
        // 费用最小正表示单位是一 tick；每机会成本为正时，费用先于此门耗尽。
        self.opportunities = quantum;
        self.started = true;
    }
    pub fn finish_round(&mut self) {
        assert!(
            self.round_complete(),
            "DRR round still has service or grants"
        );
        self.deficit = self.deficit.min(0);
        self.opportunities = 0;
        self.started = false;
    }
    /// 不补信用；正余额可同时分给多份在途 grant，退款仍归本轮。
    pub fn visit(&mut self, max_grant: u64) -> Visit {
        assert!(
            self.started && max_grant != 0,
            "DRR visit outside funded round"
        );
        if self.deficit <= 0 || self.opportunities == 0 {
            return Visit::Skip;
        }
        let ticks = self.deficit.min(i128::from(max_grant)) as u64;
        self.opportunities -= 1;
        self.deficit -= i128::from(ticks);
        self.in_flight = self
            .in_flight
            .checked_add(1)
            .expect("DRR in-flight overflow");
        Visit::Grant(Grant { ticks })
    }
    pub fn settle(&mut self, grant: Grant, actual: u64) {
        let deficit = self
            .deficit
            .checked_add(i128::from(grant.ticks) - i128::from(actual))
            .expect("DRR cost exceeds epoch range");
        self.in_flight = self
            .in_flight
            .checked_sub(1)
            .expect("DRR settle without grant");
        self.deficit = deficit;
    }
    /// 独立成本和 grant 同域线性化；不造访问、不补信用。
    pub fn charge(&mut self, actual: u64) {
        self.deficit = self
            .deficit
            .checked_sub(i128::from(actual))
            .expect("DRR cost exceeds epoch range");
    }
    pub fn absorb_debt(&mut self, child: &mut Ledger) {
        assert!(child.is_quiescent(), "debt transfer from in-flight ledger");
        if child.deficit < 0 {
            self.deficit = self
                .deficit
                .checked_add(child.deficit)
                .expect("DRR inherited debt exceeds epoch range");
        }
        child.deficit = 0;
    }
    /// 真正无 Ready/在途才退出；闲置信用不积攒，债保留。
    pub fn idle(&mut self) {
        assert!(self.is_quiescent(), "idle ledger has unsettled grants");
        self.deficit = self.deficit.min(0);
        self.opportunities = 0;
        self.started = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn grant(l: &mut Ledger, cap: u64) -> Grant {
        let Visit::Grant(g) = l.visit(cap) else {
            panic!("grant missing")
        };
        g
    }
    #[test]
    fn unequal_atomic_costs_receive_equal_round_service() {
        let (mut a, mut b) = (Ledger::new(), Ledger::new());
        let (mut sa, mut sb) = (0, 0);
        for _ in 0..100 {
            a.begin_round(100);
            b.begin_round(100);
            while !a.round_complete() {
                let g = grant(&mut a, 100);
                a.settle(g, 1);
                sa += 1;
            }
            let g = grant(&mut b, 100);
            b.settle(g, 100);
            sb += 100;
            assert!(b.round_complete());
            assert_eq!(sa, sb);
            a.finish_round();
            b.finish_round();
        }
    }
    #[test]
    fn late_refund_cannot_refill_in_flight_round() {
        let mut l = Ledger::new();
        l.begin_round(100);
        let a = grant(&mut l, 50);
        let b = grant(&mut l, 50);
        assert_eq!(l.visit(50), Visit::Skip);
        assert!(!l.round_complete());
        l.settle(a, 10);
        let c = grant(&mut l, 50);
        assert_eq!(c.ticks(), 40);
        l.settle(b, 70);
        assert!(!l.round_complete());
        l.settle(c, 40);
        assert_eq!(l.deficit(), -20);
        assert!(l.round_complete());
        l.finish_round();
        l.begin_round(100);
        assert_eq!(l.deficit(), 80);
    }
    #[test]
    fn direct_cost_and_child_debt_commute_with_in_flight_settlement() {
        for settle_first in [false, true] {
            let mut p = Ledger::new();
            p.begin_round(100);
            let mut g = Some(grant(&mut p, 60));
            let mut c = Ledger::new();
            c.begin_round(20);
            let cg = grant(&mut c, 20);
            c.settle(cg, 35);
            if settle_first {
                p.settle(g.take().unwrap(), 90);
            }
            let count = p.in_flight();
            p.charge(7);
            p.absorb_debt(&mut c);
            assert_eq!(p.in_flight(), count);
            assert_eq!(c.deficit(), 0);
            if let Some(g) = g {
                p.settle(g, 90);
            }
            assert_eq!(p.deficit(), -12);
            assert!(p.round_complete());
        }
    }
    #[test]
    fn overrun_requires_distinct_repayment_rounds() {
        let mut l = Ledger::new();
        l.begin_round(4);
        let g = grant(&mut l, 4);
        l.settle(g, 10);
        l.finish_round();
        l.begin_round(4);
        assert_eq!(l.visit(4), Visit::Skip);
        assert_eq!(l.deficit(), -2);
        l.finish_round();
        l.begin_round(4);
        let g = grant(&mut l, 4);
        assert_eq!(g.ticks(), 2);
        l.settle(g, 2);
        l.finish_round();
    }
    #[test]
    fn idle_keeps_debt_and_discards_unused_credit() {
        let mut l = Ledger::new();
        l.begin_round(10);
        let g = grant(&mut l, 4);
        l.settle(g, 1);
        l.idle();
        assert_eq!(l.deficit(), 0);
        assert!(!l.round_started());
        l.charge(30);
        l.idle();
        l.begin_round(20);
        assert_eq!(l.deficit(), -10);
        assert!(l.round_complete());
    }
    #[test]
    fn tick_cost_is_not_truncated_at_signed_64_bit_limit() {
        let mut l = Ledger::new();
        l.begin_round(1);
        let g = grant(&mut l, 1);
        l.settle(g, u64::MAX);
        assert_eq!(l.deficit(), 1 - i128::from(u64::MAX));
        let mut p = Ledger::new();
        p.absorb_debt(&mut l);
        assert_eq!(p.deficit(), 1 - i128::from(u64::MAX));
    }
    #[test]
    #[should_panic(expected = "DRR round already active")]
    fn no_next_credit_before_late_grant_settlement() {
        let mut l = Ledger::new();
        l.begin_round(1);
        let _g = grant(&mut l, 1);
        l.begin_round(1);
    }
    #[test]
    fn blocked_round_does_not_keep_idle_credit() {
        let mut l = Ledger::new();
        l.begin_round(100);
        let g = grant(&mut l, 10);
        l.settle(g, 0);
        l.idle();
        assert_eq!(l.deficit(), 0);
        l.begin_round(100);
        assert_eq!(l.deficit(), 100);
    }

    #[test]
    fn zero_cost_accounts_finish_each_finite_round_without_fictitious_cost() {
        let mut accounts = [Ledger::new(), Ledger::new()];
        let mut steps = [0; 2];
        for _ in 0..3 {
            for (index, account) in accounts.iter_mut().enumerate() {
                account.begin_round(2);
                for _ in 0..2 {
                    let g = grant(account, 1);
                    account.settle(g, 0);
                    steps[index] += 1;
                }
                assert_eq!(account.deficit(), 2);
                assert_eq!(account.visit(1), Visit::Skip);
                assert!(account.round_complete());
                account.finish_round();
                assert_eq!(account.deficit(), 0);
            }
        }
        assert_eq!(steps, [6, 6]);
    }

    #[test]
    fn opportunity_limit_waits_for_late_refunds_and_keeps_overrun_debt() {
        for last_cost in [0, 5] {
            let mut l = Ledger::new();
            l.begin_round(2);
            let a = grant(&mut l, 1);
            let b = grant(&mut l, 1);
            l.settle(a, 0);
            assert!(!l.round_complete());
            assert_eq!(l.visit(1), Visit::Skip);
            l.settle(b, last_cost);
            assert_eq!(l.deficit(), 2 - i128::from(last_cost));
            assert!(l.round_complete());
            l.finish_round();
            assert_eq!(l.deficit(), if last_cost == 0 { 0 } else { -3 });
        }
    }

    #[test]
    fn mixed_zero_cost_round_preserves_actual_cost_until_service_is_discarded() {
        let mut l = Ledger::new();
        l.begin_round(4);
        for actual in [0, 1, 0, 1] {
            let g = grant(&mut l, 1);
            l.settle(g, actual);
        }
        assert_eq!(l.deficit(), 2);
        assert!(l.round_complete());
        l.finish_round();
        assert_eq!(l.deficit(), 0);
        l.begin_round(4);
        assert_eq!(l.deficit(), 4, "unused service must not accumulate");
    }
}
