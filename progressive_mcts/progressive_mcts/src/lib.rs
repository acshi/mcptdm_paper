pub mod klucb;
use serde::Deserialize;

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CostBoundMode {
    Normal,
    LowerBound,
    Marginal,
}

impl std::fmt::Display for CostBoundMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Normal => write!(f, "normal"),
            Self::LowerBound => write!(f, "lower_bound"),
            Self::Marginal => write!(f, "marginal"),
        }
    }
}

impl std::str::FromStr for CostBoundMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "normal" => Ok(Self::Normal),
            "lower_bound" => Ok(Self::LowerBound),
            "marginal" => Ok(Self::Marginal),
            _ => Err(format!("Invalid CostBoundMode '{}'", s)),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChildSelectionMode {
    UCB,
    UCBV,
    UCBd,
    KLUCB,
    #[serde(rename = "klucb+")]
    KLUCBP,
}

impl std::fmt::Display for ChildSelectionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UCB => write!(f, "ucb"),
            Self::UCBV => write!(f, "ucbv"),
            Self::UCBd => write!(f, "ucbd"),
            Self::KLUCB => write!(f, "klucb"),
            Self::KLUCBP => write!(f, "klucb+"),
        }
    }
}

impl std::str::FromStr for ChildSelectionMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "ucb" => Ok(Self::UCB),
            "ucbv" => Ok(Self::UCBV),
            "ucbd" => Ok(Self::UCBd),
            "klucb" => Ok(Self::KLUCB),
            "klucb+" => Ok(Self::KLUCBP),
            _ => Err(format!("Invalid ChildSelectionMode '{}'", s)),
        }
    }
}