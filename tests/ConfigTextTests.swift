// Tests for ConfigText.swift (shared `expect` harness from tests/DNSGuardTests.swift).
import Foundation

func runConfigTextTests() {
    // --- which reader answers, and what is remembered -------------------------
    var fileReads = 0, helperReads = 0
    var fileText: String? = "[Interface]\nDNS = 10.0.0.22\n"
    var stamp: Date? = Date(timeIntervalSince1970: 1)
    let reader = ConfigReader(read: { _ in fileReads += 1; return fileText },
                              viaHelper: { _ in helperReads += 1; return "[Interface]\nDNS = 10.0.0.4\n" },
                              modified: { _ in stamp })

    expect(reader.text("t"), "[Interface]\nDNS = 10.0.0.22\n", "config: the file is read while it opens")
    expect(helperReads, 0, "config: no helper is run while the file opens")

    expect(reader.text("t"), "[Interface]\nDNS = 10.0.0.22\n", "config: the same text comes back")
    expect(fileReads, 1, "config: the text is cached while the file's date is unchanged")

    fileText = "[Interface]\nDNS = 10.0.0.5\n"
    stamp = Date(timeIntervalSince1970: 2)
    expect(reader.text("t"), "[Interface]\nDNS = 10.0.0.5\n", "config: a newer date re-reads the file")

    // --- a root-owned config this user cannot open ----------------------------
    var rootHelperReads = 0
    let rootOwned = ConfigReader(read: { _ in nil },
                                 viaHelper: { _ in rootHelperReads += 1; return "[Interface]\nDNS = 10.0.0.4\n" },
                                 modified: { _ in Date(timeIntervalSince1970: 3) })
    expect(rootOwned.text("t"), "[Interface]\nDNS = 10.0.0.4\n", "config: the helper answers for a root-owned file")
    _ = rootOwned.text("t")
    expect(rootHelperReads, 1, "config: the helper's answer is cached too")

    // Without a date there is nothing to notice a change by, so nothing is kept.
    var datelessReads = 0
    let dateless = ConfigReader(read: { _ in nil },
                                viaHelper: { _ in datelessReads += 1; return "[Interface]\n" },
                                modified: { _ in nil })
    _ = dateless.text("t"); _ = dateless.text("t")
    expect(datelessReads, 2, "config: a config with no readable date is not cached")

    let nothing = ConfigReader(read: { _ in nil }, viaHelper: { _ in nil }, modified: { _ in nil })
    expect(nothing.text("t"), nil, "config: nil when neither the file nor the helper answers")
}
