import Foundation
import Testing
import UserNotifications
@testable import Fusebox

@Suite("Push relay")
struct PushRelayTests {
    // The vector `the_sealed_format_matches_the_apps_test_vector` in src/push/relay.rs checks too.
    static let key = Data(base64Encoded: "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=")!
    static let sealed = "ZGVmZ2hpamtsbW5vMzmqDw2FM7wEQByEuxAOmGKxb23lQZoch7TUOJLRwCy2xWKiI3biJsfRDUyYMPuPfiP5ilAmJAaZnsH2geTA7nusqGexJVJr7E+ELTHrVocNFjEWi+f9p6uZFP5ZMGEw7RL9CwU5y1HQlr+wxmiVnVFO8i7vE8pb3Fu/k7HdZqMQpbrbAFUsQqdreifl5Srn+uXHZDU59LAdsw4/j2fMnvVDBWayv42BHjzE1cBWwq31nrjHQ5Fo7RttcD2m5TK7uUp024Z/B24sXUdBMVKAbSEstDilO2Dch3xn9pGSp6gJOzXHramsjmeW4IC5"
    static let plaintext = ##"{"title":"Claude sign-in expired","body":"user1@example.com needs signing in again.","path":"#/accounts/claude-user1.json","kind":"signin","level":"err","provider":"claude","account":"claude-user1.json","thread":"claude"}"##

    /// What the relay sends (src/push/relay.rs, `send_one`).
    static func relayed(sealed: String = sealed) -> UNMutableNotificationContent {
        let c = UNMutableNotificationContent()
        c.title = "Fusebox"
        c.body = "Open Fusebox to see what happened."
        c.threadIdentifier = "fusebox"
        c.badge = 2
        c.userInfo = [
            "aps": ["alert": ["title": "Fusebox", "body": "Open Fusebox to see what happened."], "mutable-content": 1,
                    "badge": 2, "thread-id": "fusebox"],
            "sealed": sealed, "v": 1,
        ]
        return c
    }

    @Test func theSharedVectorOpens() throws {
        let opened = try SealedPush.open(Self.sealed, key: Self.key)
        let expected = try JSONDecoder().decode(SealedPush.Contents.self, from: Data(Self.plaintext.utf8))
        #expect(opened == expected)
        #expect(opened.title == "Claude sign-in expired")
        #expect(opened.body == "user1@example.com needs signing in again.")
        #expect(opened.path == "#/accounts/claude-user1.json")
        #expect(opened.thread == "claude")
    }

    @Test func anotherKeyOrATamperedBoxDoesNotOpen() {
        #expect(throws: (any Error).self) { try SealedPush.open(Self.sealed, key: Data(repeating: 7, count: 32)) }
        var bytes = Data(base64Encoded: Self.sealed)!
        bytes[20] ^= 1
        #expect(throws: (any Error).self) { try SealedPush.open(bytes.base64EncodedString(), key: Self.key) }
        #expect(throws: (any Error).self) { try SealedPush.open("not base64!", key: Self.key) }
        #expect(throws: (any Error).self) { try SealedPush.open("AAAA", key: Self.key) }
    }

    @Test func revealShowsTheContentsAndRoutesLikeADirectPush() throws {
        let out = SealedPush.reveal(Self.relayed(), key: Self.key)
        #expect(out.title == "Claude sign-in expired")
        #expect(out.body == "user1@example.com needs signing in again.")
        #expect(out.threadIdentifier == "claude")
        #expect(out.badge == 2)
        #expect(out.userInfo["sealed"] == nil)
        #expect(out.userInfo["path"] as? String == "#/accounts/claude-user1.json")
        #expect(out.userInfo["kind"] as? String == "signin")
        #expect(out.userInfo["level"] as? String == "err")
        #expect(out.userInfo["provider"] as? String == "claude")
        #expect(out.userInfo["account"] as? String == "claude-user1.json")
        #expect(DeepLink.route(path: out.userInfo["path"] as? String) == .account("claude-user1.json"))
    }

    @Test func withoutTheKeyOrAWrongOneTheGenericAlertStays() {
        for key in [nil, Data(repeating: 1, count: 32), Data(repeating: 1, count: 5)] as [Data?] {
            let out = SealedPush.reveal(Self.relayed(), key: key)
            #expect(out.title == "Fusebox")
            #expect(out.body == "Open Fusebox to see what happened.")
            #expect(out.threadIdentifier == "fusebox")
        }
        let garbled = SealedPush.reveal(Self.relayed(sealed: "%%%"), key: Self.key)
        #expect(garbled.title == "Fusebox")
    }

    @Test func aDirectNotificationPassesThrough() {
        let c = UNMutableNotificationContent()
        c.title = "Codex limit reached"
        c.body = "Every account is used up."
        c.userInfo = ["path": "#/accounts/codex-1.json", "kind": "provider"]
        let out = SealedPush.reveal(c, key: Self.key)
        #expect(out === c)
    }

    @Test func aKeyIs32RandomBytes() throws {
        let a = try #require(RelayDevice.makeKey()), b = try #require(RelayDevice.makeKey())
        #expect(a.count == 32)
        #expect(a != b)
        let device = RelayDevice(relay: URL(string: "https://push.whombat.com")!, handle: "h", key: Self.key, token: "t")
        #expect(device.keyBase64 == "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=")
        let again = try JSONDecoder().decode(RelayDevice.self, from: JSONEncoder().encode(device))
        #expect(again == device)
    }

    // MARK: Which path, for each shape of GET /api/push

    static func status(_ json: String) throws -> PushStatus {
        try JSONDecoder.fusebox.decode(PushStatus.self, from: Data(json.utf8))
    }

    @Test func apnsSetMeansDirect() throws {
        #expect(PushPath.choose(try Self.status(#"{"public_key":"B","subscriptions":[],"apns":true,"relay":"https://push.whombat.com"}"#)) == .direct)
        #expect(PushPath.choose(try Self.status(#"{"public_key":"B","subscriptions":[],"apns":true,"relay":null}"#)) == .direct)
        // A server from before the relay, with apns set.
        #expect(PushPath.choose(try Self.status(#"{"public_key":"B","subscriptions":[],"apns":true}"#)) == .direct)
    }

    @Test func aRelayURLWithoutApnsMeansTheRelay() throws {
        let s = try Self.status(#"{"public_key":"B","subscriptions":[{"id":"a","label":"Phone","kind":"apns","via":"relay"}],"apns":false,"relay":"https://push.whombat.com"}"#)
        #expect(PushPath.choose(s) == .relay(URL(string: "https://push.whombat.com")!))
        #expect(s.subscriptions[0].via == "relay")
    }

    @Test func noRelayAndNoApnsMeansUnavailable() throws {
        #expect(PushPath.choose(try Self.status(#"{"public_key":"B","subscriptions":[],"apns":false,"relay":null}"#)) == .unavailable)
        #expect(PushPath.choose(try Self.status(#"{"public_key":"B","subscriptions":[],"apns":false,"relay":""}"#)) == .unavailable)
        #expect(PushPath.choose(try Self.status(#"{"public_key":"B","subscriptions":[],"apns":false,"relay":"not a url"}"#)) == .unavailable)
    }

    @Test func olderServersWithoutTheFieldsAreUnavailable() throws {
        // Before APNs support: neither field.
        #expect(PushPath.choose(try Self.status(#"{"public_key":"B","subscriptions":[]}"#)) == .unavailable)
        #expect(PushPath.choose(try Self.status(#"{"public_key":"B","subscriptions":[],"apns":false}"#)) == .unavailable)
        #expect(PushPath.choose(try decode(PushStatus.self, "push")) == .unavailable)
    }

    @Test func theRelayRegistrationIsWhatFuseboxReads() throws {
        let body = RelayRegistration(relay: .init(handle: "h", key: "k"), label: "Phone")
        let json = try JSONSerialization.jsonObject(with: JSONEncoder.fusebox.encode(body)) as? [String: Any]
        #expect((json?["relay"] as? [String: String]) == ["handle": "h", "key": "k"])
        #expect(json?["label"] as? String == "Phone")
        #expect(json?["token"] == nil)
    }
}
