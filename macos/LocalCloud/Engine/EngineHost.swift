import Foundation

/// Where the engine keeps its state, and where it keeps the files.
///
/// Android needed a whole object here, because it needed a multicast lock
/// before mDNS would hear anything. The Mac needs no such thing, so what is
/// left is the pair of directories - a platform decision rather than an engine
/// one - and the one rule that goes with each.
enum EngineHost {

    /// The engine's own state: this device's private key, the catalog, and
    /// every block this device holds.
    ///
    /// Kept out of backup, deliberately. Migration Assistant onto a second Mac
    /// would otherwise carry this device's identity and certificate across, and
    /// two members of the mesh with one id and one certificate is not a device
    /// that moved but a device that forked. Recovering a Mac is what pairing
    /// and pulling copies are for, and they do it without ever duplicating an
    /// identity.
    static let stateDirectory: URL = applicationSupport
        .appending(path: "Engine", directoryHint: .isDirectory)

    /// What this device holds, as files.
    ///
    /// The container's Documents folder: a real folder a person can open. On a
    /// desktop the engine watches it, so a file put there through Finder is
    /// indexed and offered to the mesh without this app being involved at all.
    /// That is the difference between this and the phone, where the folder is
    /// app-private and nothing but the app can reach it.
    static let syncDirectory: URL = FileManager.default
        .urls(for: .documentDirectory, in: .userDomainMask)[0]

    /// Constructs the one engine this process has.
    ///
    /// One per process, for the life of the process: the engine holds the
    /// database, the block store and the identity, so a second instance over
    /// the same directory would be a second writer.
    ///
    /// Nothing here names the device. `whoami` reads the computer name through
    /// SystemConfiguration on a Mac and gets the real one, so unlike Android -
    /// which reports the literal string "Unknown" and had to be told - this
    /// device already knows what it is called. Renaming stays a thing a person
    /// can do; it is not something the app has to do on first run.
    static func make() throws -> Engine {
        try prepareStateDirectory()
        return try Engine(
            baseDir: stateDirectory.path(percentEncoded: false),
            syncDirPath: syncDirectory.path(percentEncoded: false)
        )
    }

    /// The engine creates this itself, but only after this app has had the
    /// chance to say it should not be backed up - and a directory has to exist
    /// before anything can be said about it.
    private static func prepareStateDirectory() throws {
        try FileManager.default.createDirectory(
            at: stateDirectory,
            withIntermediateDirectories: true
        )
        var directory = stateDirectory
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? directory.setResourceValues(values)
    }

    private static var applicationSupport: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
    }
}
