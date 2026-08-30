//! 未发布页表 eager range builder 测试。

use page_table::{
    ENTRIES, EagerMapper, FrameExhausted, FrameMemory, FrameNumber, MapError, Ppn, Pte,
    ReservedTableFrame, Vpn, flags, pages_at,
};

struct ReservedFrame(FrameNumber);

impl ReservedTableFrame for ReservedFrame {
    fn number(&self) -> FrameNumber {
        self.0
    }

    fn commit(self) -> FrameNumber {
        self.0
    }
}

struct Tables {
    tables: Vec<[Pte; ENTRIES]>,
    limit: usize,
}

impl Tables {
    fn new(limit: usize) -> Self {
        Self {
            tables: vec![[Pte::invalid(); ENTRIES]],
            limit,
        }
    }

    fn mapped(&self, vpn: Vpn) -> Option<(Ppn, usize)> {
        let mut frame = FrameNumber(0);
        for level in (0..3).rev() {
            let entry = self.tables[frame.0][vpn.index_at(level)];
            if entry.is_leaf() {
                return Some((entry.ppn() + vpn.0 % pages_at(level), level));
            }
            if !entry.is_branch() {
                return None;
            }
            frame = entry.next_frame();
        }
        None
    }
}

impl FrameMemory for Tables {
    type ReservedFrame = ReservedFrame;

    fn reserve_frame(&mut self) -> Result<Self::ReservedFrame, FrameExhausted> {
        if self.tables.len() - 1 == self.limit {
            return Err(FrameExhausted);
        }
        let frame = FrameNumber(self.tables.len());
        self.tables.push([Pte::invalid(); ENTRIES]);
        Ok(ReservedFrame(frame))
    }

    fn free_frame(&mut self, _frame: FrameNumber) {
        panic!("eager test tables are never reclaimed")
    }

    fn table_mut(&mut self, frame: FrameNumber) -> &mut [Pte; ENTRIES] {
        &mut self.tables[frame.0]
    }
}

#[test]
fn selects_largest_legal_leaf_for_each_segment() {
    let mut tables = Tables::new(8);
    {
        let mut mapper = EagerMapper::<_, 3>::new(&mut tables, FrameNumber(0));
        mapper
            .map_range(Vpn(0), pages_at(2), Ppn(0), flags::KERNEL_DIRECT)
            .unwrap();

        let second_gib = pages_at(2);
        mapper
            .map_range(
                Vpn(second_gib + 1),
                pages_at(1) - 1,
                Ppn(second_gib + 1),
                flags::KERNEL_DIRECT,
            )
            .unwrap();
        mapper
            .map_range(
                Vpn(second_gib + pages_at(1)),
                pages_at(1),
                Ppn(second_gib + pages_at(1)),
                flags::KERNEL_DIRECT,
            )
            .unwrap();
    }

    assert_eq!(tables.mapped(Vpn(123)), Some((Ppn(123), 2)));
    assert_eq!(
        tables.mapped(Vpn(pages_at(2) + 1)),
        Some((Ppn(pages_at(2) + 1), 0))
    );
    assert_eq!(
        tables.mapped(Vpn(pages_at(2) + pages_at(1) + 17)),
        Some((Ppn(pages_at(2) + pages_at(1) + 17), 1))
    );
    assert_eq!(
        tables.tables.len(),
        3,
        "root plus one middle and one leaf table"
    );
}

#[test]
fn preserves_a_matching_coarse_leaf_for_an_idempotent_subrange() {
    let mut tables = Tables::new(0);
    {
        let mut mapper = EagerMapper::<_, 3>::new(&mut tables, FrameNumber(0));
        mapper
            .map_range(Vpn(0), pages_at(2), Ppn(0), flags::KERNEL_DIRECT)
            .unwrap();
        mapper
            .map_range(Vpn(17), 1, Ppn(17), flags::KERNEL_DIRECT)
            .unwrap();
    }
    assert_eq!(tables.tables.len(), 1);
    assert_eq!(tables.mapped(Vpn(17)), Some((Ppn(17), 2)));
}

#[test]
fn leaves_explicit_holes_unmapped() {
    let mut tables = Tables::new(8);
    let hole = 3 * pages_at(1);
    {
        let mut mapper = EagerMapper::<_, 3>::new(&mut tables, FrameNumber(0));
        mapper
            .map_range(Vpn(0), hole, Ppn(0), flags::KERNEL_DIRECT)
            .unwrap();
        mapper
            .map_range(
                Vpn(hole + 1),
                pages_at(2) - hole - 1,
                Ppn(hole + 1),
                flags::KERNEL_DIRECT,
            )
            .unwrap();
    }

    assert_eq!(tables.mapped(Vpn(hole - 1)), Some((Ppn(hole - 1), 1)));
    assert_eq!(tables.mapped(Vpn(hole)), None);
    assert_eq!(tables.mapped(Vpn(hole + 1)), Some((Ppn(hole + 1), 0)));
}

#[test]
fn validates_conflicts_before_mutating_the_range() {
    let mut tables = Tables::new(8);
    {
        let mut mapper = EagerMapper::<_, 3>::new(&mut tables, FrameNumber(0));
        mapper
            .map_range(Vpn(1), 1, Ppn(1), flags::KERNEL_DIRECT)
            .unwrap();
        assert!(matches!(
            mapper.map_range(Vpn(1), 1, Ppn(2), flags::KERNEL_DIRECT),
            Err(MapError::Conflict { vpn: Vpn(1) })
        ));
    }
    assert_eq!(tables.mapped(Vpn(1)), Some((Ppn(1), 0)));
}

#[test]
fn reports_static_table_budget_exhaustion() {
    let mut tables = Tables::new(1);
    let mut mapper = EagerMapper::<_, 3>::new(&mut tables, FrameNumber(0));
    assert_eq!(
        mapper.map_range(Vpn(1), 1, Ppn(1), flags::KERNEL_DIRECT),
        Err(MapError::FrameExhausted)
    );
}
