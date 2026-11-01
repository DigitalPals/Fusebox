import XCTest

/// Drives the app against a real Fusebox and saves screenshots. Run with:
///
///   TEST_RUNNER_FUSEBOX_KEY_FILE=~/.config/fusebox-ios/management-key \
///   TEST_RUNNER_FUSEBOX_SHOTS=/tmp/fusebox-shots TEST_RUNNER_FUSEBOX_SHOT_PREFIX=pro \
///   xcodebuild test -scheme FuseboxUITests -destination 'platform=iOS Simulator,name=iPhone 18 Pro'
///
/// The key file's path goes to the app (Debug builds read it once into the Keychain); the key
/// itself never passes through the test.
@MainActor
final class ScreenshotTests: XCTestCase {
    private var env: [String: String] { ProcessInfo.processInfo.environment }
    private var app: XCUIApplication!

    override func setUp() async throws {
        continueAfterFailure = false
        app = XCUIApplication()
        if let key = env["FUSEBOX_KEY_FILE"] { app.launchEnvironment["FUSEBOX_DEBUG_KEY_FILE"] = key }
        if let url = env["FUSEBOX_URL"] { app.launchEnvironment["FUSEBOX_DEBUG_URL"] = url }
        if env["FUSEBOX_REGIONS"] == "1" { app.launchEnvironment["FUSEBOX_DEBUG_REGIONS"] = "1" }
        if let p = env["FUSEBOX_OUTAGE"] { app.launchEnvironment["FUSEBOX_DEBUG_OUTAGE"] = p }
        if env["FUSEBOX_FORCE_RELAY"] == "1" { app.launchEnvironment["FUSEBOX_DEBUG_FORCE_RELAY"] = "1" }
    }

    private func shot(_ name: String) {
        let dir = env["FUSEBOX_SHOTS"]
        let prefix = env["FUSEBOX_SHOT_PREFIX"].map { "\($0)-" } ?? ""
        let file = "\(prefix)\(name).png"
        if let dir, env["FUSEBOX_HOST_SHOTS"] == "1" {
            // A foldable's inner panel isn't XCUIScreen.main: ask the Mac (scripts/ui-screenshots) to
            // take it with `simctl io screenshot --display`, and wait until it has.
            try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
            let request = URL(fileURLWithPath: dir).appendingPathComponent("request-\(file)")
            try? Data().write(to: request)
            for _ in 0..<40 where FileManager.default.fileExists(atPath: request.path) { usleep(250_000) }
            return
        }
        let png = XCUIScreen.main.screenshot().pngRepresentation
        let attachment = XCTAttachment(data: png, uniformTypeIdentifier: "public.png")
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
        guard let dir else { return }
        try? FileManager.default.createDirectory(atPath: dir, withIntermediateDirectories: true)
        try? png.write(to: URL(fileURLWithPath: dir).appendingPathComponent(file))
    }

    private func waitForData() {
        let live = app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS 'Live'")).firstMatch
        XCTAssertTrue(live.waitForExistence(timeout: 40), "no live connection to the server")
        sleep(3)
    }

    private var isPhone: Bool { app.tabBars.firstMatch.exists }

    /// Every main screen, in whichever layout the size class gives.
    func testTour() throws {
        app.launch()
        waitForData()
        // The scene restores where it was left; start the tour on the Overview.
        if !app.descendants(matching: .any)["combined-weekly"].waitForExistence(timeout: 5) {
            if isPhone { app.tabBars.buttons["Overview"].tap() } else { showSidebar(); app.buttons["sidebar-overview"].firstMatch.tap(); hideSidebar() }
        }
        XCTAssertTrue(app.descendants(matching: .any)["combined-weekly"].waitForExistence(timeout: 20))
        sleep(1)
        shot("01-overview")

        if isPhone {
            app.swipeUp()
            shot("02-overview-scrolled")
            app.swipeDown(); app.swipeDown()
            app.tabBars.buttons["Accounts"].tap()
            XCTAssertTrue(app.descendants(matching: .any)["accounts"].waitForExistence(timeout: 10))
            shot("03-accounts")
            firstElement("account-row-").tap()
            XCTAssertTrue(app.descendants(matching: .any)["account-detail"].waitForExistence(timeout: 10))
            sleep(2)
            shot("04-account-detail")
            app.swipeUp()
            shot("05-account-detail-scrolled")
            app.tabBars.buttons["Requests"].tap()
            sleep(2)
            shot("06-requests")
            firstElement("request-").tap()
            XCTAssertTrue(app.descendants(matching: .any)["request-detail"].waitForExistence(timeout: 10))
            shot("07-request-detail")
            app.tabBars.buttons["Usage"].tap()
            _ = app.descendants(matching: .any)["usage-total"].waitForExistence(timeout: 20)
            sleep(2)
            shot("08-usage")
            app.swipeUp()
            shot("09-usage-breakdown")
            app.tabBars.buttons["Settings"].tap()
            sleep(2)
            shot("10-settings")
            app.tabBars.buttons["Overview"].tap()
            app.buttons["faults-button"].firstMatch.tap()
            sleep(1)
            shot("11-faults")
        } else {
            showSidebar()
            shot("02-sidebar")
            app.buttons["sidebar-accounts"].firstMatch.tap()
            sleep(2)
            shot("02b-accounts-list")
            firstElement("account-row-").tap()
            XCTAssertTrue(app.descendants(matching: .any)["account-detail"].waitForExistence(timeout: 10))
            sleep(2)
            shot("03-accounts-detail")
            showSidebar()
            app.buttons["sidebar-requests"].firstMatch.tap()
            sleep(2)
            firstElement("request-").tap()
            XCTAssertTrue(app.descendants(matching: .any)["request-detail"].waitForExistence(timeout: 10))
            shot("04-requests-inspector")
            showSidebar()
            app.buttons["sidebar-usage"].firstMatch.tap()
            _ = app.descendants(matching: .any)["usage-total"].waitForExistence(timeout: 20)
            sleep(2)
            shot("05-usage")
            showSidebar()
            app.buttons["sidebar-faults"].firstMatch.tap()
            sleep(2)
            shot("06-faults")
            showSidebar()
            app.buttons["sidebar-overview"].firstMatch.tap()
            sleep(1)
            shot("07-overview-again")
        }
    }

    private func firstElement(_ prefix: String) -> XCUIElement {
        let e = app.buttons.matching(NSPredicate(format: "identifier BEGINSWITH %@", prefix)).firstMatch
        let any = e.waitForExistence(timeout: 10) ? e : app.descendants(matching: .any).matching(NSPredicate(format: "identifier BEGINSWITH %@", prefix)).firstMatch
        XCTAssertTrue(any.waitForExistence(timeout: 10), "no \(prefix)… element")
        return any
    }

    private func hideSidebar() {
        guard app.buttons["sidebar-overview"].firstMatch.isHittable else { return }
        let toggle = app.navigationBars.buttons.matching(NSPredicate(format: "label CONTAINS[c] 'sidebar' OR identifier CONTAINS[c] 'sidebar' OR label CONTAINS[c] 'Hide'")).firstMatch
        if toggle.exists { toggle.tap(); sleep(1) }
    }

    private func showSidebar() {
        if app.buttons["sidebar-overview"].firstMatch.isHittable { return }
        let toggle = app.navigationBars.buttons.matching(NSPredicate(format: "label CONTAINS[c] 'sidebar' OR identifier CONTAINS[c] 'sidebar' OR label CONTAINS[c] 'Show'")).firstMatch
        if toggle.exists { toggle.tap(); sleep(1) }
    }

    /// Turns notifications on, sends a real test through Fusebox and APNs, and taps it.
    /// The test notification's path is the dashboard's notifications page, which opens the Overview.
    func testPushRegistrationAndTestNotification() throws {
        app.launch()
        waitForData()
        goToSettings()
        let toggle = app.switches["push-toggle"].firstMatch
        XCTAssertTrue(toggle.waitForExistence(timeout: 10))
        let flip = { toggle.switches.firstMatch.exists ? toggle.switches.firstMatch.tap() : toggle.tap() }
        if (toggle.value as? String) == "1" {
            // Already on from an earlier run: turn it off (DELETE) to register afresh.
            flip()
            XCTAssertTrue(app.staticTexts["Notifications are off for this device."].waitForExistence(timeout: 20))
        }
        do {
            flip()
            let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
            let allow = springboard.buttons["Allow"]
            if allow.waitForExistence(timeout: 8) { allow.tap() }
        }
        let message = app.staticTexts["push-message"]
        XCTAssertTrue(message.waitForExistence(timeout: 30))
        shot("push-01-on")
        XCTAssertEqual(message.label, "Notifications are on for this device.", "registration: \(message.label)")
        let via = app.descendants(matching: .any)["push-via"]
        XCTAssertTrue(via.waitForExistence(timeout: 5))
        print("Delivery: \(via.label)")

        // The test arrives within seconds as a system banner over the app (Settings is showing).
        // Tapping it routes by its path, the dashboard's notifications page: the Overview.
        app.buttons["push-test"].firstMatch.tap()
        let banner = banner(containing: "Notifications are on", timeout: 45)
        shot("push-02-banner")
        open(banner)
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 10))
        XCTAssertTrue(app.descendants(matching: .any)["overview"].waitForExistence(timeout: 10))
        shot("push-03-opened")
    }

    /// Waits for a notification (sent from the Mac with `xcrun simctl push`) whose path is an
    /// account, taps it, and expects that account's page.
    func testNotificationTapOpensAccount() throws {
        app.launch()
        waitForData()
        XCUIDevice.shared.press(.home)
        // Sent from the Mac while this waits: xcrun simctl push … with "path": "#/accounts/<id>".
        let banner = banner(containing: "Routing check", timeout: 120)
        shot("route-01-banner")
        open(banner)
        XCTAssertTrue(app.descendants(matching: .any)["account-detail"].waitForExistence(timeout: 15))
        sleep(1)
        shot("route-02-account")
    }

    /// The relay path end to end, with FUSEBOX_FORCE_RELAY=1: registers with the real push
    /// relay (development) without telling the server, waits on the home screen for a sealed
    /// notification sent from the Mac (`scripts/relay-e2e send`), expects the notification service
    /// to have decrypted it, taps it, expects that account's page, and turns notifications off,
    /// which removes the handle from the relay.
    func testRelayNotificationDecryptsAndOpensAccount() throws {
        XCTAssertEqual(app.launchEnvironment["FUSEBOX_DEBUG_FORCE_RELAY"], "1", "run with FUSEBOX_FORCE_RELAY=1")
        app.launch()
        waitForData()
        goToSettings()
        let toggle = app.switches["push-toggle"].firstMatch
        XCTAssertTrue(toggle.waitForExistence(timeout: 10))
        let flip = { toggle.switches.firstMatch.exists ? toggle.switches.firstMatch.tap() : toggle.tap() }
        if (toggle.value as? String) == "1" {
            flip()
            XCTAssertTrue(app.staticTexts["Notifications are off for this device."].waitForExistence(timeout: 20))
        }
        flip()
        let allow = XCUIApplication(bundleIdentifier: "com.apple.springboard").buttons["Allow"]
        if allow.waitForExistence(timeout: 8) { allow.tap() }
        let message = app.staticTexts["push-message"]
        XCTAssertTrue(message.waitForExistence(timeout: 30))
        XCTAssertEqual(message.label, "Notifications are on for this device.", "registration: \(message.label)")
        let via = app.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS 'push relay (encrypted)'")).firstMatch
        XCTAssertTrue(via.waitForExistence(timeout: 5), "Settings doesn't say the relay is in use")
        shot("relay-01-on")

        XCUIDevice.shared.press(.home)
        // Sent from the Mac while this waits; the title is only in the sealed part.
        let banner = banner(containing: env["FUSEBOX_RELAY_TITLE"] ?? "Relay check", timeout: 180)
        shot("relay-02-banner")
        XCTAssertFalse(banner.label.contains("Open Fusebox to see what happened"), "not decrypted: \(banner.label)")
        open(banner)
        XCTAssertTrue(app.descendants(matching: .any)["account-detail"].waitForExistence(timeout: 15))
        sleep(1)
        shot("relay-03-account")

        goToSettings()
        XCTAssertTrue(toggle.waitForExistence(timeout: 10))
        if (toggle.value as? String) == "1" { flip() }
        XCTAssertTrue(app.staticTexts["Notifications are off for this device."].waitForExistence(timeout: 20))
    }

    /// A notification in Notification Center, pulled down from the top of the home screen.
    private func notification(containing text: String, timeout: TimeInterval) -> XCUIElement {
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let predicate = NSPredicate(format: "label CONTAINS %@", text)
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            let top = springboard.coordinate(withNormalizedOffset: CGVector(dx: 0.25, dy: 0.002))
            top.press(forDuration: 0.1, thenDragTo: springboard.coordinate(withNormalizedOffset: CGVector(dx: 0.25, dy: 0.7)))
            let hit = springboard.descendants(matching: .any).matching(predicate).firstMatch
            if hit.waitForExistence(timeout: 4) { return hit }
            XCUIDevice.shared.press(.home)
            sleep(3)
        }
        XCTFail("no notification containing “\(text)”")
        return springboard
    }

    /// A notification banner over the home screen.
    private func banner(containing text: String, timeout: TimeInterval) -> XCUIElement {
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let hit = springboard.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS %@", text)).firstMatch
        XCTAssertTrue(hit.waitForExistence(timeout: timeout), "no notification containing “\(text)”")
        return hit
    }

    /// Taps a notification until its app comes forward (the first tap can only select it).
    private func open(_ notification: XCUIElement) {
        let wasForeground = app.state == .runningForeground
        notification.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        if !wasForeground {
            for _ in 0..<2 where !app.wait(for: .runningForeground, timeout: 4) {
                notification.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
            }
        }
    }

    /// With FUSEBOX_OUTAGE=<provider> (a synthetic provider-exhausted fault, Debug builds only):
    /// the Live Activity counting down to the first account back, on the cover sheet.
    func testOutageLiveActivity() throws {
        app.launch()
        waitForData()
        shot("la-01-app")
        XCUIDevice.shared.press(.home)
        sleep(2)
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        springboard.coordinate(withNormalizedOffset: CGVector(dx: 0.25, dy: 0.002))
            .press(forDuration: 0.1, thenDragTo: springboard.coordinate(withNormalizedOffset: CGVector(dx: 0.25, dy: 0.7)))
        sleep(2)
        shot("la-02-cover-sheet")
        XCUIDevice.shared.press(.home)
    }

    /// Adds the Combined weekly widget to the home screen through the widget gallery and
    /// photographs it. Springboard's gallery changes between iOS versions: best effort.
    func testAddWidget() throws {
        app.launch()
        waitForData()
        XCUIDevice.shared.press(.home)
        sleep(2)
        let sb = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        sb.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.6)).press(forDuration: 2.0)
        sleep(2)
        shot("widget-01-jiggle")
        let edit = sb.buttons.matching(NSPredicate(format: "label == 'Edit' OR label == 'Add Widget' OR identifier CONTAINS[c] 'add'")).firstMatch
        if edit.waitForExistence(timeout: 5) { edit.tap(); sleep(1) }
        let add = sb.buttons.matching(NSPredicate(format: "label CONTAINS[c] 'Add Widget'")).firstMatch
        if add.waitForExistence(timeout: 3) { add.tap(); sleep(2) }
        shot("widget-02-gallery")
        let search = sb.searchFields.firstMatch
        if search.waitForExistence(timeout: 5) { search.tap(); search.typeText("Fusebox"); sleep(2) }
        shot("widget-03-search")
        let result = sb.staticTexts.matching(NSPredicate(format: "label == 'Fusebox'")).firstMatch
        if result.waitForExistence(timeout: 5) { result.tap(); sleep(4) }
        shot("widget-04-fusebox")
        sb.swipeLeft(); sleep(3)
        shot("widget-04b-fusebox")
        sb.swipeRight(); sleep(2)
        let addButton = sb.buttons.matching(NSPredicate(format: "label CONTAINS[c] 'Add Widget'")).firstMatch
        if addButton.waitForExistence(timeout: 5) { addButton.tap(); sleep(3) }
        XCUIDevice.shared.press(.home)
        sleep(4)
        shot("widget-05-home")
    }

    private func goToSettings() {
        if isPhone {
            app.tabBars.buttons["Settings"].tap()
        } else {
            showSidebar()
            app.buttons.matching(NSPredicate(format: "label == 'Settings'")).firstMatch.tap()
        }
        sleep(1)
    }
}
