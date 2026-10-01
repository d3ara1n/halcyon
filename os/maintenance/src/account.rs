//! 资金账户 DRR 账本。账户环与 Ready 队列由内核存储；本模块只持每账户
//! 的有符号赤字与在途数，提供“访问一次（有界推进）/ 预扣 / 结算”。
//!
//! 赤字可为负：实际占用超过授予时转为欠账，下轮先偿还。账户有在途
//! 工作时内核必须保持账本存活，不因 Ready 暂空重置。

/// 一次访问的结果。
#[must_use = "a granted visit must be settled"]
#[derive(Debug, PartialEq, Eq)]
pub enum Visit {
    /// 已预扣 `grant` tick，可摘取该账户队首记录。
    Grant(Grant),
    /// 赤字仍非正，已补一次量子；仲裁推进到下一个账户。
    Skip,
}

/// 预扣的授予额度，结算时归还。
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
    deficit: i64,
    in_flight: u32,
}

impl Default for Ledger {
    fn default() -> Self {
        Self::new()
    }
}

impl Ledger {
    pub const fn new() -> Self {
        Self { deficit: 0, in_flight: 0 }
    }

    pub const fn deficit(&self) -> i64 {
        self.deficit
    }

    pub const fn in_flight(&self) -> u32 {
        self.in_flight
    }

    /// 账户无 Ready 且无在途时可从环移除；欠账随之保留在账本中。
    pub const fn is_quiescent(&self) -> bool {
        self.in_flight == 0
    }

    /// 环指针到达本账户时调用一次（仅当其 Ready 队列非空）。
    ///
    /// 赤字为正直接授予；否则补 `quantum`，仍非正则 Skip。单次调用至多
    /// 补一次量子，锁内不循环。`max_grant` 为单批授予上限。
    pub fn visit(&mut self, quantum: u64, max_grant: u64) -> Visit {
        assert!(quantum > 0 && max_grant > 0, "DRR quantum and grant must be nonzero");
        if self.deficit <= 0 {
            self.deficit = self.deficit.saturating_add(to_i64(quantum));
            if self.deficit <= 0 {
                return Visit::Skip;
            }
        }
        let ticks = (self.deficit as u64).min(max_grant);
        self.deficit -= to_i64(ticks);
        self.in_flight = self.in_flight.checked_add(1).expect("DRR in-flight overflow");
        Visit::Grant(Grant { ticks })
    }

    /// 批次结束：退还未用部分，超出部分记为欠账。
    pub fn settle(&mut self, grant: Grant, actual: u64) {
        self.in_flight = self.in_flight.checked_sub(1).expect("DRR settle without grant");
        let diff = to_i64(grant.ticks) - to_i64(actual);
        self.deficit = self.deficit.saturating_add(diff);
    }

    /// 子账户销毁时并入其剩余负 deficit（独立 debt transfer）：不动
    /// in_flight、不伪造 grant；正信用不转移。`child` 必须已无在途。
    pub fn absorb_debt(&mut self, child: &mut Ledger) {
        assert!(child.is_quiescent(), "debt transfer from in-flight ledger");
        if child.deficit < 0 {
            self.deficit = self.deficit.saturating_add(child.deficit);
        }
        child.deficit = 0;
    }

    /// Ready 队列变空且无在途时调用：不积攒正信用，保留欠账。
    pub fn idle(&mut self) {
        if self.deficit > 0 {
            self.deficit = 0;
        }
    }
}

fn to_i64(v: u64) -> i64 {
    i64::try_from(v).expect("tick value exceeds ledger range")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 销毁子账户不得抹去欠账（轮次 3 结论）；正信用不随转移放大父份额。
    #[test]
    fn debt_transfer_keeps_debt_drops_credit() {
        let (mut p, mut c) = (Ledger::new(), Ledger::new());
        let Visit::Grant(g) = c.visit(10, 10) else { panic!() };
        c.settle(g, 100);
        p.absorb_debt(&mut c);
        assert_eq!((p.deficit(), p.in_flight(), c.deficit()), (-90, 0, 0));
        let Visit::Grant(g) = c.visit(10, 4) else { panic!() };
        c.settle(g, 0);
        p.absorb_debt(&mut c);
        assert_eq!(p.deficit(), -90);
    }

    #[test]
    fn second_grant_waits_for_refill() {
        let mut l = Ledger::new();
        let Visit::Grant(a) = l.visit(10, 10) else { panic!() };
        assert_eq!(a.ticks(), 10);
        // 赤字已为 0：补一次量子后再授予，总在途不超过两量子。
        let Visit::Grant(b) = l.visit(10, 10) else { panic!() };
        assert_eq!(l.in_flight(), 2);
        assert_eq!(l.deficit(), 0);
        l.settle(a, 10);
        l.settle(b, 10);
    }

    #[test]
    fn overrun_becomes_debt_and_is_repaid() {
        let mut l = Ledger::new();
        let Visit::Grant(g) = l.visit(4, 4) else { panic!() };
        l.settle(g, 10); // 超出 6
        assert_eq!(l.deficit(), -6);
        assert_eq!(l.visit(4, 4), Visit::Skip);
        let Visit::Grant(g) = l.visit(4, 4) else { panic!() };
        assert_eq!(g.ticks(), 2);
        l.settle(g, 2);
    }

    #[test]
    fn idle_drops_credit_keeps_debt() {
        let mut l = Ledger::new();
        let Visit::Grant(g) = l.visit(10, 4) else { panic!() };
        l.settle(g, 1);
        assert!(l.deficit() > 0);
        l.idle();
        assert_eq!(l.deficit(), 0);
        let Visit::Grant(g) = l.visit(1, 1) else { panic!() };
        l.settle(g, 5);
        l.idle();
        assert_eq!(l.deficit(), -4);
    }

    #[test]
    fn many_records_do_not_amplify_share() {
        // 两账户持续竞争，A 有大量记录、B 一条：按访问次数轮转后占用相当。
        let (mut a, mut b) = (Ledger::new(), Ledger::new());
        let (mut sa, mut sb) = (0u64, 0u64);
        for _ in 0..1000 {
            if let Visit::Grant(g) = a.visit(3, 3) {
                let t = g.ticks();
                a.settle(g, t + 1); // 每批超出一个原子
                sa += t + 1;
            }
            if let Visit::Grant(g) = b.visit(3, 3) {
                let t = g.ticks();
                b.settle(g, t);
                sb += t;
            }
        }
        assert!(sa.abs_diff(sb) <= 8, "share drift {sa} vs {sb}");
    }
}
