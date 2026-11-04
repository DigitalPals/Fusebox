import ActivityKit
import Foundation

/// Starts a Live Activity when every account of a provider is out (the `provider` fault),
/// counting down to the first one back, and ends it when the provider recovers.
/// Local updates only: the app updates it while it runs, the countdown runs by itself.
@MainActor
final class OutageActivities {
    private var ending: Set<String> = []
    /// The latest pass. Each new one cancels it and waits for it to stop, so
    /// two passes never interleave and a pass for faults that are no longer
    /// current can't update or start an activity.
    private var task: Task<Void, Never>?

    func sync(faults: [Fault], enabled: Bool) {
        let previous = task
        previous?.cancel()
        task = Task {
            await previous?.value
            guard !Task.isCancelled else { return }
            await apply(faults: faults, enabled: enabled)
        }
    }

    /// Ends every outage activity at once: after sign-out, or on another
    /// server, they describe a server the app no longer shows.
    func endAll() {
        let previous = task
        previous?.cancel()
        task = Task {
            await previous?.value
            for activity in Activity<OutageAttributes>.activities {
                await activity.end(nil, dismissalPolicy: .immediate)
            }
            ending.removeAll()
        }
    }

    private func apply(faults: [Fault], enabled: Bool) async {
        let outages = faults.filter { $0.kind == "provider" }
        let byProvider = Dictionary(outages.map { ($0.provider ?? $0.key, $0) }, uniquingKeysWith: { a, _ in a })
        let running = Activity<OutageAttributes>.activities

        for activity in running where activity.activityState == .active {
            if Task.isCancelled { return }
            let provider = activity.attributes.provider
            if let fault = byProvider[provider], enabled {
                let state = Self.state(for: fault)
                if activity.content.state != state {
                    await activity.update(ActivityContent(state: state, staleDate: fault.until))
                }
            } else if !ending.contains(activity.id) {
                ending.insert(activity.id)
                let name = activity.attributes.providerName
                let final = OutageAttributes.ContentState(
                    backAt: nil, title: enabled ? "\(name) is back" : "\(name) outage", detail: enabled ? "An account can take requests again." : "", recovered: true)
                await activity.end(ActivityContent(state: final, staleDate: nil),
                                   dismissalPolicy: enabled ? .after(.now.addingTimeInterval(15 * 60)) : .immediate)
            }
        }

        guard !Task.isCancelled, enabled, ActivityAuthorizationInfo().areActivitiesEnabled else { return }
        for (provider, fault) in byProvider where !running.contains(where: { $0.attributes.provider == provider && $0.activityState == .active }) {
            if Task.isCancelled { return }
            let attributes = OutageAttributes(provider: provider, providerName: fault.providerName ?? Provider.name(provider))
            _ = try? Activity.request(attributes: attributes,
                                      content: ActivityContent(state: Self.state(for: fault), staleDate: fault.until),
                                      pushType: nil)
        }
    }

    static func state(for fault: Fault) -> OutageAttributes.ContentState {
        OutageAttributes.ContentState(
            backAt: fault.until,
            title: fault.title,
            detail: fault.until == nil ? "Until an account is signed in again" : "Until the first account is back")
    }
}
