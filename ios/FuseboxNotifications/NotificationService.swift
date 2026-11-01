import UserNotifications

/// Opens notifications that came through the Fusebox push relay: their `sealed` field holds
/// the title, body and route, encrypted with the key this device gave its Fusebox (in the shared
/// keychain group). Without the key, or when it doesn't open, the relay's generic alert stays.
/// Direct notifications pass through unchanged.
final class NotificationService: UNNotificationServiceExtension {
    override func didReceive(_ request: UNNotificationRequest,
                             withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void) {
        let content = request.content
        guard content.userInfo["sealed"] != nil else { return contentHandler(content) }
        contentHandler(SealedPush.reveal(content, key: RelayDevice.stored?.key))
    }

    // Opening a notification is synchronous and quick; there is nothing better to show late.
    override func serviceExtensionTimeWillExpire() {}
}
