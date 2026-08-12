pub use strum::*;

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, EnumIter, EnumCount, EnumString, AsRefStr, Display)]
#[strum(serialize_all = "camelCase")]
#[repr(usize)]
pub enum ARKit {
    // Eyes Left (0-6)
    EyeBlinkLeft = 0,
    EyeLookDownLeft = 1,
    EyeLookInLeft = 2,
    EyeLookOutLeft = 3,
    EyeLookUpLeft = 4,
    EyeSquintLeft = 5,
    EyeWideLeft = 6,

    // Eyes Right (7-13)
    EyeBlinkRight = 7,
    EyeLookDownRight = 8,
    EyeLookInRight = 9,
    EyeLookOutRight = 10,
    EyeLookUpRight = 11,
    EyeSquintRight = 12,
    EyeWideRight = 13,

    // Jaw (14-17)
    JawForward = 14,
    JawLeft = 15,
    JawRight = 16,
    JawOpen = 17,

    // Mouth (18-40)
    MouthClose = 18,
    MouthFunnel = 19,
    MouthPucker = 20,
    MouthLeft = 21,
    MouthRight = 22,
    MouthSmileLeft = 23,
    MouthSmileRight = 24,
    MouthFrownLeft = 25,
    MouthFrownRight = 26,
    MouthDimpleLeft = 27,
    MouthDimpleRight = 28,
    MouthStretchLeft = 29,
    MouthStretchRight = 30,
    MouthRollLower = 31,
    MouthRollUpper = 32,
    MouthShrugLower = 33,
    MouthShrugUpper = 34,
    MouthPressLeft = 35,
    MouthPressRight = 36,
    MouthLowerDownLeft = 37,
    MouthLowerDownRight = 38,
    MouthUpperUpLeft = 39,
    MouthUpperUpRight = 40,

    // Eyebrows (41-45)
    BrowDownLeft = 41,
    BrowDownRight = 42,
    BrowInnerUp = 43,
    BrowOuterUpLeft = 44,
    BrowOuterUpRight = 45,

    // Cheeks (46-48)
    CheekPuff = 46,
    CheekSquintLeft = 47,
    CheekSquintRight = 48,

    // Nose (49-50)
    NoseSneerLeft = 49,
    NoseSneerRight = 50,

    // Tongue (51)
    TongueOut = 51,
}

impl ARKit {
    /// Convert to index (0-51)
    pub fn as_index(self) -> usize {
        let result = self as usize;
        assert!(result < 52);
        result
    }

    /// Convert from index (0-51)
    pub fn from_index(index: usize) -> Option<Self> {
        Self::iter().nth(index)
    }
}

/// Tongue morph target shapes
/// Based on Audio2Face 3D SDK tongue blend shapes
/// These represent the different morph targets available for the tongue mesh
#[derive(Debug, Copy, Clone, PartialEq, Eq, EnumIter, EnumCount, Display, EnumString, AsRefStr)]
#[strum(serialize_all = "camelCase")]
#[repr(usize)]
pub enum TongueShape {
    // Tip movements (0-3)
    TongueTipUp = 0,
    TongueTipDown = 1,
    TongueTipLeft = 2,
    TongueTipRight = 3,

    // Roll movements (4-7)
    TongueRollUp = 4,
    TongueRollDown = 5,
    TongueRollLeft = 6,
    TongueRollRight = 7,

    // Position movements (8-12)
    TongueUp = 8,
    TongueDown = 9,
    TongueLeft = 10,
    TongueRight = 11,
    TongueIn = 12,

    // Shape deformations (13-15)
    TongueStretch = 13,
    TongueWide = 14,
    TongueNarrow = 15,
}

impl TongueShape {
    /// Convert to tongue morph target index
    pub fn as_index(self) -> usize {
        let result = self as usize;
        assert!(result < 16);
        result
    }

    /// Convert from index to tongue morph target
    pub fn from_index(index: usize) -> Option<Self> {
        Self::iter().nth(index)
    }
}
