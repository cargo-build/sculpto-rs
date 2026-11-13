//! Body-part roles and the required-part contract for a character bundle.
//!
//! A character is assembled from named parts — body, hands, feet, head and any
//! garments. [`ShapeRole`] names each part and carries the Skyrim SE biped
//! partition it defaults to, so a writer can place the part in the slot the
//! game expects. [`validate_required_roles`] is the required-part gate: a
//! complete character needs body, hands, feet and head, and the refusal names
//! every missing role rather than stopping at the first.
//!
//! The partition defaults are the Skyrim SE biped slots, measured against the
//! supplied fixtures: `HIMBO Body - Vanilla.nif` torso uses 32, `HIMBO Hands.nif`
//! uses 33, `HIMBO Feet.nif` and `CBBE Feet.nif` use 37, and `HeadNord.nif`
//! encodes the head slot 30 (its cap variants 130/230 reduce to 30). Garments
//! default to the torso slot 32; a garment that covers another slot overrides
//! its partition explicitly.

use std::fmt;

use crate::mesh::PartitionId;

/// One named part of a character bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ShapeRole {
    /// The torso/body surface sliders deform.
    Body,
    /// The hands.
    Hands,
    /// The feet.
    Feet,
    /// The replaceable head.
    Head,
    /// A garment or outfit piece.
    Garment,
}

impl ShapeRole {
    /// Every role, in assembly order.
    pub const ALL: [ShapeRole; 5] = [
        ShapeRole::Body,
        ShapeRole::Hands,
        ShapeRole::Feet,
        ShapeRole::Head,
        ShapeRole::Garment,
    ];

    /// The Skyrim SE biped partition this part defaults to.
    ///
    /// Head 30, Body 32, Hands 33, Feet 37; a garment defaults to the torso
    /// slot 32 and overrides it when it covers another slot.
    pub fn default_partition(self) -> PartitionId {
        match self {
            ShapeRole::Head => PartitionId(30),
            ShapeRole::Body | ShapeRole::Garment => PartitionId(32),
            ShapeRole::Hands => PartitionId(33),
            ShapeRole::Feet => PartitionId(37),
        }
    }

    /// Stable label for errors and reference output.
    pub fn as_str(self) -> &'static str {
        match self {
            ShapeRole::Body => "body",
            ShapeRole::Hands => "hands",
            ShapeRole::Feet => "feet",
            ShapeRole::Head => "head",
            ShapeRole::Garment => "garment",
        }
    }

    /// Parse a stable label; unknown names are `None`, never a silent default.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "body" => Some(ShapeRole::Body),
            "hands" => Some(ShapeRole::Hands),
            "feet" => Some(ShapeRole::Feet),
            "head" => Some(ShapeRole::Head),
            "garment" => Some(ShapeRole::Garment),
            _ => None,
        }
    }
}

impl fmt::Display for ShapeRole {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// The roles a complete character must carry; garments are optional.
pub const REQUIRED_ROLES: [ShapeRole; 4] = [
    ShapeRole::Body,
    ShapeRole::Hands,
    ShapeRole::Feet,
    ShapeRole::Head,
];

/// The required roles absent from a bundle, in assembly order.
///
/// Names every missing role, not just the first, so one refusal reports the
/// whole gap.
pub fn missing_required_roles(present: &[ShapeRole]) -> Vec<ShapeRole> {
    REQUIRED_ROLES
        .iter()
        .copied()
        .filter(|role| !present.contains(role))
        .collect()
}

/// Why a role assignment cannot form a complete character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleError {
    /// One or more required roles are absent; every missing role is named.
    MissingRoles(Vec<ShapeRole>),
}

impl fmt::Display for RoleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RoleError::MissingRoles(roles) => {
                let names: Vec<&str> = roles.iter().map(|role| role.as_str()).collect();
                write!(formatter, "missing required part(s): {}", names.join(", "))
            }
        }
    }
}

impl std::error::Error for RoleError {}

/// Gate the required-part contract: body, hands, feet and head must all be
/// present. A garment is optional and never required.
///
/// # Errors
/// [`RoleError::MissingRoles`] names every required role the bundle lacks.
pub fn validate_required_roles(present: &[ShapeRole]) -> Result<(), RoleError> {
    let missing = missing_required_roles(present);
    if missing.is_empty() {
        Ok(())
    } else {
        Err(RoleError::MissingRoles(missing))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_defaults_match_the_measured_skyrim_slots() {
        assert_eq!(ShapeRole::Head.default_partition(), PartitionId(30));
        assert_eq!(ShapeRole::Body.default_partition(), PartitionId(32));
        assert_eq!(ShapeRole::Hands.default_partition(), PartitionId(33));
        assert_eq!(ShapeRole::Feet.default_partition(), PartitionId(37));
        assert_eq!(ShapeRole::Garment.default_partition(), PartitionId(32));
    }

    #[test]
    fn labels_roundtrip_and_unknown_is_none() {
        for role in ShapeRole::ALL {
            assert_eq!(ShapeRole::parse(role.as_str()), Some(role));
        }
        assert_eq!(ShapeRole::parse("horn"), None);
    }

    #[test]
    fn complete_character_passes_and_garment_is_optional() {
        validate_required_roles(&[
            ShapeRole::Body,
            ShapeRole::Hands,
            ShapeRole::Feet,
            ShapeRole::Head,
        ])
        .expect("all required roles present");
        validate_required_roles(&[
            ShapeRole::Body,
            ShapeRole::Hands,
            ShapeRole::Feet,
            ShapeRole::Head,
            ShapeRole::Garment,
        ])
        .expect("a garment does not break the contract");
    }

    #[test]
    fn every_missing_role_is_named() {
        assert_eq!(
            validate_required_roles(&[ShapeRole::Body]),
            Err(RoleError::MissingRoles(vec![
                ShapeRole::Hands,
                ShapeRole::Feet,
                ShapeRole::Head,
            ]))
        );
        assert_eq!(
            missing_required_roles(&[]),
            vec![
                ShapeRole::Body,
                ShapeRole::Hands,
                ShapeRole::Feet,
                ShapeRole::Head
            ]
        );
        assert_eq!(missing_required_roles(&REQUIRED_ROLES), Vec::new());
    }
}
