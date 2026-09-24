use libbudget::Taxonomy;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceResource {
    Authority,
    Registration,
    Bytes,
    WaitSource,
}

impl Taxonomy for ServiceResource {
    const COUNT: usize = 4;

    fn slot(self) -> usize {
        match self {
            Self::Authority => 0,
            Self::Registration => 1,
            Self::Bytes => 2,
            Self::WaitSource => 3,
        }
    }
}
