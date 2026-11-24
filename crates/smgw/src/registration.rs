/// Reference-slot spacing in the BSI TR-03109-1 v2.0 billing profiles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Period {
    FiveMinutes,
    QuarterHour,
    Hour,
}

impl Period {
    pub const fn seconds(self) -> i64 {
        match self {
            Self::FiveMinutes => 300,
            Self::QuarterHour => 900,
            Self::Hour => 3600,
        }
    }
}
