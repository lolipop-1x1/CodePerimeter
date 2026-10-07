import AppKit
import UserNotifications
import Darwin

private struct Request: Decodable {
    let version: Int
    let operation: String
    let title: String?
    let body: String?
    let target: Target?
    let localization: Localization?
}

private struct Localization: Decodable {
    let locale: String
    let detailAction: String
    let alertsAction: String
    let activationFailedTitle: String
    let activationFailedBody: String
    let acknowledge: String

    enum CodingKeys: String, CodingKey {
        case locale, acknowledge
        case detailAction = "detail_action"
        case alertsAction = "alerts_action"
        case activationFailedTitle = "activation_failed_title"
        case activationFailedBody = "activation_failed_body"
    }

    // 旧协议发送的是中文通知；保留它的按钮与点击失败提示。
    static let legacy = Localization(locale: "", detailAction: "查看详情", alertsAction: "查看告警",
        activationFailedTitle: "未能打开 CodePerimeter 告警",
        activationFailedBody: "请从本机启动 CodePerimeter 控制台并检查后台安装状态。通知中的记录可能已过期或清除。",
        acknowledge: "知道了")

    var valid: Bool {
        !locale.isEmpty && locale.utf8.count <= 64
            && locale.utf8.allSatisfy { (65...90).contains($0) || (97...122).contains($0)
                || (48...57).contains($0) || $0 == 45 || $0 == 95 }
            && [detailAction, alertsAction, activationFailedTitle, acknowledge]
                .allSatisfy { !$0.isEmpty && $0.utf8.count <= 512 }
            && !activationFailedBody.isEmpty && activationFailedBody.utf8.count <= 4096
    }

    var fields: [String: String] {
        ["locale": locale, "detail_action": detailAction, "alerts_action": alertsAction,
            "activation_failed_title": activationFailedTitle,
            "activation_failed_body": activationFailedBody, "acknowledge": acknowledge]
    }

    var alertCategory: String { locale.isEmpty ? "project_alert" : "project_alert_\(locale)" }
    var summaryCategory: String { locale.isEmpty ? "project_summary" : "project_summary_\(locale)" }

    static func from(_ fields: Any?) -> Localization {
        guard let fields = fields as? [String: String],
            let bytes = try? JSONSerialization.data(withJSONObject: fields),
            let value = try? JSONDecoder().decode(Localization.self, from: bytes),
            value.valid else { return .legacy }
        return value
    }
}

private struct Target: Decodable {
    let kind: String
    let id: String?

    var valid: Bool {
        if kind == "alerts" { return id == nil }
        return kind == "alert" && id.map(validAlertID) == true
    }
}

private func validAlertID(_ value: String) -> Bool {
    let allowed = Set("abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._:-".utf8)
    return !value.isEmpty && value.utf8.count <= 512
        && value.utf8.allSatisfy { allowed.contains($0) }
}

private func hasRequestInput() -> Bool {
    if CommandLine.arguments.contains("--request-stdin") { return true }
    // 旧版 Rust 使用 stdin 管道且没有标志，更新助手时仍接受这种发送请求。
    var metadata = stat()
    return fstat(STDIN_FILENO, &metadata) == 0
        && metadata.st_mode & mode_t(S_IFMT) == mode_t(S_IFIFO)
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
    private var receivedActivation = false
    private var activeLaunchers = 0
    private let lifecycle = NSLock()
    private var pendingActivations = 0
    private var pendingFinish: (() -> Void)?
    private let requestMode = hasRequestInput()

    func applicationWillFinishLaunching(_ notification: Notification) {
        guard Bundle.main.bundleIdentifier == "com.codeperimeter.notifications" else { return }
        // 冷启动的点击响应也需要在启动完成前注册 delegate。
        center = UNUserNotificationCenter.current()
        center.delegate = self
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard Bundle.main.bundleIdentifier == "com.codeperimeter.notifications" else {
            finish(false, "invalid_bundle")
            return
        }
        // 通知中心重新启动应用时没有 stdin 请求，等待 didReceive，不能按空请求退出。
        if !requestMode {
            DispatchQueue.main.asyncAfter(deadline: .now() + 20) {
                if !self.receivedActivation { exit(0) }
            }
            return
        }
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
                !body.isEmpty, body.utf8.count <= 8192,
                request.target?.valid != false, request.localization?.valid != false else {
                finish(false, "invalid_request")
                return
            }
            let localization = request.localization ?? .legacy
            registerCategories(localization) {
                self.center.getNotificationSettings { settings in
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
                        if let target = request.target, target.kind == "alert", let id = target.id {
                            content.userInfo = ["alert_id": id, "localization": localization.fields]
                            content.categoryIdentifier = localization.alertCategory
                        } else {
                            content.userInfo = ["localization": localization.fields]
                            content.categoryIdentifier = localization.summaryCategory
                        }
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
            }
        default:
            finish(false, "invalid_request")
        }
    }

    private func registerCategories(_ localization: Localization, completion: @escaping () -> Void) {
        // 不覆盖其他语言的类别；已送达通知的操作按钮保留原语言。
        center.getNotificationCategories { existing in
            var categories = existing.filter { $0.identifier != localization.alertCategory
                && $0.identifier != localization.summaryCategory }
            categories.insert(UNNotificationCategory(identifier: localization.alertCategory, actions: [
                UNNotificationAction(identifier: "view_detail", title: localization.detailAction, options: [.foreground])
            ], intentIdentifiers: []))
            categories.insert(UNNotificationCategory(identifier: localization.summaryCategory, actions: [
                UNNotificationAction(identifier: "view_detail", title: localization.alertsAction, options: [.foreground])
            ], intentIdentifiers: []))
            self.center.setNotificationCategories(categories)
            completion()
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
            self.lifecycle.lock()
            if self.pendingActivations > 0 {
                if self.pendingFinish == nil { self.pendingFinish = { self.finish(ok, code, settings) } }
                self.lifecycle.unlock()
                return
            }
            self.lifecycle.unlock()
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
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void) {
        lifecycle.lock()
        pendingActivations += 1
        lifecycle.unlock()
        DispatchQueue.main.async { self.activate(response, completionHandler) }
    }

    private func activate(_ response: UNNotificationResponse,
        _ completionHandler: @escaping () -> Void) {
        defer {
            lifecycle.lock()
            pendingActivations -= 1
            let finish = pendingActivations == 0 ? pendingFinish : nil
            if pendingActivations == 0 { pendingFinish = nil }
            let idle = pendingActivations == 0 && activeLaunchers == 0
            lifecycle.unlock()
            finish?()
            if !requestMode && idle { exit(0) }
        }
        guard response.actionIdentifier == UNNotificationDefaultActionIdentifier
            || response.actionIdentifier == "view_detail" else {
            completionHandler()
            return
        }
        receivedActivation = true
        let fields = response.notification.request.content.userInfo
        let localization = Localization.from(fields["localization"])
        if let value = fields["alert_id"] {
            guard let id = value as? String, validAlertID(id) else {
                activationFailed(completionHandler, localization)
                return
            }
        }
        let arguments = (fields["alert_id"] as? String).map { ["ui", "--alert-id=\($0)"] }
            ?? ["ui", "--alerts"]
        // 不从通知元数据接受 URL、令牌或可执行路径，启动位置只由本机 UID 决定。
        let executable = "/Library/CodePerimeter/\(getuid())/codeperimeter"
        guard getuid() != 0, trustedLauncher(executable) else {
            activationFailed(completionHandler, localization)
            return
        }
        let task = Process()
        task.executableURL = URL(fileURLWithPath: executable)
        task.arguments = arguments
        task.standardInput = FileHandle.nullDevice
        task.standardOutput = FileHandle.nullDevice
        task.standardError = FileHandle.nullDevice
        do {
            // 在主队列上完成启动，再允许发送回执退出，避免点击任务还未创建就退出。
            try task.run()
            if requestMode {
                // 发送实例只确认点击已启动；网页启动器继续运行，不延长通知提交期限。
                completionHandler()
                DispatchQueue.global(qos: .utility).async { task.waitUntilExit() }
                return
            }
            activeLaunchers += 1
            DispatchQueue.global(qos: .userInitiated).async {
                DispatchQueue.main.asyncAfter(deadline: .now() + 12) {
                    if task.isRunning { task.terminate() }
                }
                task.waitUntilExit()
                DispatchQueue.main.async {
                    self.activeLaunchers -= 1
                    if task.terminationStatus == 0 {
                        completionHandler()
                        self.lifecycle.lock()
                        let idle = self.pendingActivations == 0 && self.activeLaunchers == 0
                        self.lifecycle.unlock()
                        if idle { exit(0) }
                    } else {
                        self.activationFailed(completionHandler, localization)
                    }
                }
            }
        } catch {
            activationFailed(completionHandler, localization)
        }
    }

    private func trustedLauncher(_ executable: String) -> Bool {
        var path = executable
        while !path.isEmpty {
            var metadata = stat()
            guard lstat(path, &metadata) == 0, metadata.st_uid == 0,
                metadata.st_mode & 0o022 == 0,
                metadata.st_mode & mode_t(S_IFMT) != mode_t(S_IFLNK) else { return false }
            if path == executable {
                guard metadata.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG),
                    metadata.st_mode & 0o111 != 0 else { return false }
            } else if metadata.st_mode & mode_t(S_IFMT) != mode_t(S_IFDIR) { return false }
            if path == "/" { break }
            path = (path as NSString).deletingLastPathComponent
        }
        return true
    }

    private func activationFailed(_ completionHandler: @escaping () -> Void,
        _ localization: Localization) {
        let alert = NSAlert()
        alert.messageText = localization.activationFailedTitle
        alert.informativeText = localization.activationFailedBody
        alert.addButton(withTitle: localization.acknowledge)
        NSApp.activate(ignoringOtherApps: true)
        alert.runModal()
        completionHandler()
        if !requestMode && activeLaunchers == 0 {
            lifecycle.lock()
            let idle = pendingActivations == 0
            lifecycle.unlock()
            if idle { exit(1) }
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
