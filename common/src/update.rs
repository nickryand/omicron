use std::fmt;

use crate::api::internal::nexus::KnownArtifactKind;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Description of the `artifacts.json` target found in rack update
/// repositories.
///
/// Currently, this has a single top-level field; this gives us an escape hatch
/// in the future if we need to change the schema in a non-backwards-compatible
/// way.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ArtifactsDocument {
    pub artifacts: Vec<Artifact>,
}

/// Describes an artifact available in the repository.
///
/// See also [`crate::api::internal::nexus::UpdateArtifactId`], which is used
/// internally in Nexus and Sled Agent.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Artifact {
    pub name: String,
    pub version: String,
    pub kind: ArtifactKind,
    pub target: String,
}

impl Artifact {
    /// Returns the artifact ID for this artifact.
    pub fn id(&self) -> ArtifactId {
        ArtifactId {
            name: self.name.clone(),
            version: self.version.clone(),
            kind: self.kind.clone(),
        }
    }
}

/// An identifier for an artifact.
///
/// The kind is [`ArtifactKind`], indicating that it might represent an artifact
/// whose kind is unknown.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
    Deserialize,
    Serialize,
    JsonSchema,
)]
pub struct ArtifactId {
    /// The artifact's name.
    pub name: String,

    /// The artifact's version.
    pub version: String,

    /// The kind of artifact this is.
    pub kind: ArtifactKind,
}

/// The kind of artifact we are dealing with.
///
/// To ensure older versions of Nexus can work with update repositories that
/// describe artifact kinds it is not yet aware of, this is a newtype wrapper
/// around a string. The set of known artifact kinds is described in
/// [`KnownArtifactKind`], and this type has conversions to and from it.
#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
    Deserialize,
    Serialize,
    JsonSchema,
)]
#[serde(transparent)]
pub struct ArtifactKind(String);

impl ArtifactKind {
    /// Creates a new `ArtifactKind` from a string.
    pub fn new(kind: String) -> Self {
        Self(kind)
    }

    /// Creates a new `ArtifactKind` from a known kind.
    pub fn from_known(kind: KnownArtifactKind) -> Self {
        Self(kind.to_string())
    }

    /// Returns the kind as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Converts self to a `KnownArtifactKind`, if it is known.
    pub fn to_known(&self) -> Option<KnownArtifactKind> {
        self.0.parse().ok()
    }
}

impl From<KnownArtifactKind> for ArtifactKind {
    fn from(kind: KnownArtifactKind) -> Self {
        Self::from_known(kind)
    }
}

impl fmt::Display for ArtifactKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use crate::api::internal::nexus::KnownArtifactKind;
    use crate::update::ArtifactKind;

    #[test]
    fn serde_artifact_kind() {
        assert_eq!(
            serde_json::from_str::<ArtifactKind>("\"gimlet_sp\"")
                .unwrap()
                .to_known(),
            Some(KnownArtifactKind::GimletSp)
        );
        assert_eq!(
            serde_json::from_str::<ArtifactKind>("\"fhqwhgads\"")
                .unwrap()
                .to_known(),
            None,
        );
        assert!(serde_json::from_str::<ArtifactKind>("null").is_err());

        assert_eq!(
            serde_json::to_string(&ArtifactKind::from_known(
                KnownArtifactKind::GimletSp
            ))
            .unwrap(),
            "\"gimlet_sp\""
        );
        assert_eq!(
            serde_json::to_string(&ArtifactKind::new("fhqwhgads".to_string()))
                .unwrap(),
            "\"fhqwhgads\""
        );
    }
}
