import Foundation

/// What to put on screen when the engine refuses.
///
/// The engine writes a sentence for every one of these - "That device is not
/// visible on the network" - but the sentence does not survive the crossing.
/// uniffi generates `errorDescription` as `String(reflecting: self)`, so what
/// arrives here is `localcloud.EngineError.NotVisible(deviceId: "7f3a…")`,
/// which is diagnostics rather than something to show a person. Kotlin has the
/// same problem in a milder form; Swift's is blunter, because reflection
/// prints the module and the case name too.
///
/// The typed variants do survive, and they were the point: this matches on
/// them and supplies the English on this side. Deliberately not a match on the
/// message. The variant is the contract; the wording is ours to improve.
extension EngineError {
    var sentence: String {
        switch self {
        case .NoSuchItem:
            "That item is no longer in the catalog."
        case .NotHeldHere:
            "This device does not have that file's contents."
        case .NotAHolder:
            "That device does not have a copy to work with."
        case .NotTrashed:
            "Delete the remaining copies before destroying this."
        case .InTrash:
            "That item is in the trash. Restore it first."
        case .NotPaired:
            "That device is not paired with this one."
        case .NotVisible:
            "That device is not on the network right now."
        case .InvalidName(let name):
            "“\(name)” cannot be used as a name."
        case .NoSuchFile:
            "The file could not be read."
        case .NothingSelected:
            "Choose at least one device."
        case .NoUsableDevices(let reason):
            reason
        case .NoSuchCollision:
            "That name conflict was already settled."
        case .Pairing(let reason):
            reason
        case .Internal(let reason):
            "Something failed on this device: \(reason)"
        }
    }
}

extension Error {
    /// Anything at all that went wrong, as a sentence.
    ///
    /// Engine failures get the wording above; anything else - a file the
    /// sandbox will not open, a full disk while copying - falls back to its own
    /// message rather than being reported as an engine problem it was not.
    var sentenceForUser: String {
        (self as? EngineError)?.sentence ?? localizedDescription
    }
}
