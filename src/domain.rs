use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Facet {
    Movie,
    Series,
    Anime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    Radarr,
    Sonarr,
}

impl Flavor {
    pub fn accepts(self, facet: Facet) -> bool {
        match self {
            Flavor::Radarr => facet == Facet::Movie,
            Flavor::Sonarr => matches!(facet, Facet::Series | Facet::Anime),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flavor_accepts_facets() {
        assert!(Flavor::Radarr.accepts(Facet::Movie));
        assert!(!Flavor::Radarr.accepts(Facet::Series));
        assert!(Flavor::Sonarr.accepts(Facet::Anime));
    }
}
