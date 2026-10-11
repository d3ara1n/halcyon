use libbudget::Taxonomy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceResource {
    Authority,
    Registration,
    Bytes,
    WaitSource,
    Access,
    Request,
    Outbox,
    Watch,
}

impl Taxonomy for ServiceResource {
    const COUNT: usize = 8;

    fn slot(self) -> usize {
        match self {
            Self::Authority => 0,
            Self::Registration => 1,
            Self::Bytes => 2,
            Self::WaitSource => 3,
            Self::Access => 4,
            Self::Request => 5,
            Self::Outbox => 6,
            Self::Watch => 7,
        }
    }
}
