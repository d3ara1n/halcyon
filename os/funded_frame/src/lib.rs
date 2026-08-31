//! 页额度与物理 extent 的资金化取得事务。
//!
//! 本 crate 只编排仿射 token，不认识锁、物理地址或 capability。内核适配层分别实现
//! 额度与库存端口，因此 host 测试与内核使用同一条 reserve → claim → clear → commit
//! 路径。reservation、claim 与最终 credit 的析构负责逆序回滚。

#![no_std]
#![forbid(unsafe_code)]

/// 单次资金化事务的独立页数与 extent 数上限。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub max_pages: usize,
    pub max_extents: usize,
}

/// 可在提交前自然回滚的页额度 reservation。
pub trait QuotaReservation {
    type Credit;

    /// 提交已经由物理 backing 覆盖的额度。实现必须不可失败。
    fn commit(self) -> Self::Credit;
}

/// 页额度来源。返回的 reservation 必须在析构时自动回滚。
pub trait QuotaSource {
    type Reservation: QuotaReservation;
    type Error;

    fn reserve(&self, pages: usize) -> Result<Self::Reservation, Self::Error>;
}

/// 尚未发布的物理 extent。析构必须把 extent 归还库存。
pub trait PhysicalClaim {
    fn pages(&self) -> usize;

    /// 在不持库存锁时把完整 extent 清零。实现必须不可失败。
    fn clear(&mut self);
}

/// 物理库存来源。每次取得至多 `max_pages`，不持锁跨调用。
pub trait PhysicalSource {
    type Claim: PhysicalClaim;
    type Error;

    fn claim_largest(&self, max_pages: usize) -> Result<Self::Claim, Self::Error>;
}

/// 资金化取得失败。所有变体返回时，已取得的 quota 与 extent 均已由 RAII 回滚。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FundError<Q, P> {
    ZeroPages,
    PageLimit,
    ExtentLimit,
    InvalidClaim,
    Quota(Q),
    Physical(P),
}

/// 与一组物理 extent 同寿命的页额度 credit。
///
/// 字段顺序保证自然析构先归还物理 extent，再归还额度；跨两类账本的瞬时交接始终
/// 由本对象唯一占有。
#[must_use = "funded frames must remain owned until their backing is retired"]
pub struct Funded<C, P, const N: usize> {
    claims: [Option<P>; N],
    claim_count: usize,
    pages: usize,
    credit: C,
}

impl<C, P, const N: usize> Funded<C, P, N> {
    pub const fn pages(&self) -> usize {
        self.pages
    }

    pub const fn extent_count(&self) -> usize {
        self.claim_count
    }

    pub fn claims(&self) -> impl ExactSizeIterator<Item = &P> {
        self.claims[..self.claim_count]
            .iter()
            .map(|claim| claim.as_ref().expect("funded extent slot is empty"))
    }

    pub fn credit(&self) -> &C {
        &self.credit
    }
}

/// 普通资金化事务的结果类型。
pub type FundedResult<Q, I, const N: usize> = Result<
    Funded<
        <<Q as QuotaSource>::Reservation as QuotaReservation>::Credit,
        <I as PhysicalSource>::Claim,
        N,
    >,
    FundError<<Q as QuotaSource>::Error, <I as PhysicalSource>::Error>,
>;

/// 从普通库存取得并资金化一组有界 extent。
///
/// 函数不会同时访问 quota 与 inventory：先预留额度，再以短调用逐段取得物理页，
/// 全部取得后才锁外清零，最后执行不可失败的额度提交。
pub fn fund<Q, I, const N: usize>(
    quota: &Q,
    inventory: &I,
    pages: usize,
    limits: Limits,
) -> FundedResult<Q, I, N>
where
    Q: QuotaSource,
    I: PhysicalSource,
{
    validate_request::<Q::Error, I::Error, N>(pages, limits)?;

    // declaration order is intentional: an error drops claims before rolling back quota.
    let reservation = quota.reserve(pages).map_err(FundError::Quota)?;
    let mut claims: [Option<I::Claim>; N] = core::array::from_fn(|_| None);
    let mut claim_count = 0usize;
    let mut claimed_pages = 0usize;

    while claimed_pages < pages {
        if claim_count == limits.max_extents {
            return Err(FundError::ExtentLimit);
        }
        let remaining = pages - claimed_pages;
        let claim = inventory
            .claim_largest(remaining)
            .map_err(FundError::Physical)?;
        let extent_pages = claim.pages();
        if extent_pages == 0 || extent_pages > remaining {
            return Err(FundError::InvalidClaim);
        }
        claims[claim_count] = Some(claim);
        claim_count += 1;
        claimed_pages += extent_pages;
    }

    for claim in &mut claims[..claim_count] {
        claim
            .as_mut()
            .expect("claimed extent slot is empty")
            .clear();
    }
    let credit = reservation.commit();
    Ok(Funded {
        claims,
        claim_count,
        pages,
        credit,
    })
}

fn validate_request<Q, P, const N: usize>(
    pages: usize,
    limits: Limits,
) -> Result<(), FundError<Q, P>> {
    if pages == 0 {
        return Err(FundError::ZeroPages);
    }
    if limits.max_pages == 0 || pages > limits.max_pages {
        return Err(FundError::PageLimit);
    }
    if limits.max_extents == 0 || limits.max_extents > N {
        return Err(FundError::ExtentLimit);
    }
    Ok(())
}
