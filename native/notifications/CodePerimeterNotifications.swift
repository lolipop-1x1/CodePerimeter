import AppKit
import UserNotifications
import Darwin

private struct Request: Decodable {
    let version: Int
    let operation: String
    let title: String?
    let body: String?
}

private struct Reply: Encodable {
    let version = 1
    let ok: Bool
    let code: String
    let authorization: String?
    let alertsEnabled: Bool?

    enum CodingKeys: String, CodingKey {
        case version, ok, code, authorization
        case alertsEnabled = "alerts_enabled"
    }
}

private final class NotificationApplication: NSObject, NSApplicationDelegate,
    UNUserNotificationCenterDelegate {
    private var finished = false
    private let maximumInput = 16 * 1024
    private var center: UNUserNotificationCenter!

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard Bundle.main.bundleIdentifier == "com.codeperimeter.notifications" else {
            finish(false, "invalid_bundle")
            return
        }
        center = UNUserNotificationCenter.current()
        center.delegate = self
        DispatchQueue.main.asyncAfter(deadline: .now() + 60) {
            self.finish(false, "timed_out")
        }
        // 只接受有界 stdin JSON，不把通知正文放进命令行或诊断输出。
        DispatchQueue.global(qos: .userInitiated).async {
            var input = Data()
            while input.count <= self.maximumInput {
                let chunk = FileHandle.standardInput.readData(
                    ofLength: min(4096, self.maximumInput + 1 - input.count)
                )
                if chunk.isEmpty { break }
                input.append(chunk)
            }
            guard input.count <= self.maximumInput,
                let request = try? JSONDecoder().decode(Request.self, from: input),
                request.version == 1 else {
                self.finish(false, "invalid_request")
                return
            }
            DispatchQueue.main.async { self.perform(request) }
        }
    }

    private func perform(_ request: Request) {
        if request.operation != "authorize" {
            DispatchQueue.main.asyncAfter(deadline: .now() + 5) {
                self.finish(false, "timed_out")
            }
        }
        switch request.operation {
        case "status":
            center.getNotificationSettings { settings in
                self.finish(true, "status", settings)
            }
        case "authorize":
            // 状态查询和 send 不弹权限；只有显式 authorize 请求申请授权。
            center.requestAuthorization(options: [.alert, .sound]) { granted, error in
                guard error == nil else {
                    self.finish(false, "authorization_error")
                    return
                }
                guard granted else {
                    self.finish(false, "permission_denied")
                    return
                }
                self.center.getNotificationSettings { settings in
                    self.finish(settings.alertSetting == .enabled,
                        settings.alertSetting == .enabled ? "authorized" : "alerts_disabled",
                        settings)
                }
            }
        case "send":
            guard let title = request.title, let body = request.body,
                !title.isEmpty, title.utf8.count <= 512,
                !body.isEmpty, body.utf8.count <= 8192 else {
                finish(false, "invalid_request")
                return
            }
            center.getNotificationSettings { settings in
                switch settings.authorizationStatus {
                case .notDetermined:
                    self.finish(false, "authorization_required", settings)
                case .denied:
                    self.finish(false, "permission_denied", settings)
                case .authorized, .provisional:
                    guard settings.alertSetting == .enabled else {
                        self.finish(false, "alerts_disabled", settings)
                        return
                    }
                    let content = UNMutableNotificationContent()
                    content.title = title
                    content.body = body
                    content.sound = .default
                    let notification = UNNotificationRequest(
                        identifier: UUID().uuidString, content: content, trigger: nil
                    )
                    self.center.add(notification) { error in
                        // add 回执只证明系统接受提交，不能证明用户已看到通知。
                        self.finish(error == nil,
                            error == nil ? "accepted" : "submission_failed", settings)
                    }
                @unknown default:
                    self.finish(false, "unsupported_authorization", settings)
                }
            }
        default:
            finish(false, "invalid_request")
        }
    }

    private func authorization(_ settings: UNNotificationSettings) -> String {
        switch settings.authorizationStatus {
        case .notDetermined: return "not_determined"
        case .denied: return "denied"
        case .authorized: return "authorized"
        case .provisional: return "provisional"
        @unknown default: return "unknown"
        }
    }

    private func finish(_ ok: Bool, _ code: String,
        _ settings: UNNotificationSettings? = nil) {
        DispatchQueue.main.async {
            guard !self.finished else { return }
            self.finished = true
            let response = Reply(ok: ok, code: code,
                authorization: settings.map { self.authorization($0) },
                alertsEnabled: settings.map { $0.alertSetting == .enabled })
            guard var bytes = try? JSONEncoder().encode(response) else { exit(1) }
            bytes.append(10)
            FileHandle.standardOutput.write(bytes)
            exit(ok ? 0 : 1)
        }
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler:
            @escaping (UNNotificationPresentationOptions) -> Void) {
        completionHandler([.banner, .list, .sound])
    }
}

let application = NSApplication.shared
private let notificationApplication = NotificationApplication()
application.setActivationPolicy(.accessory)
application.delegate = notificationApplication
application.run()
