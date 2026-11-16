import Cocoa
import WebKit

private let serviceLabel = "local.model-access.hub"

final class LoomConfig {
    let dataDir: URL
    let gatewayPort: UInt16
    let adminPort: UInt16
    let customBin: String?

    init() {
        let env = ProcessInfo.processInfo.environment
        let home = FileManager.default.homeDirectoryForCurrentUser
        if let dir = env["MODEL_HUB_DATA_DIR"], !dir.isEmpty {
            self.dataDir = URL(fileURLWithPath: dir)
        } else {
            self.dataDir = home
                .appendingPathComponent("Library/Application Support/ModelHub/.local")
        }
        self.gatewayPort = UInt16(env["LOOM_GATEWAY_PORT"] ?? "") ?? 4319
        self.adminPort = UInt16(env["LOOM_ADMIN_PORT"] ?? "") ?? 4320
        self.customBin = env["LOOM_BIN"].flatMap { $0.isEmpty ? nil : $0 }
    }

    func resolveBinary() -> String? {
        let fm = FileManager.default
        let home = fm.homeDirectoryForCurrentUser.path
        var candidates: [String] = []
        if let custom = customBin {
            candidates.append(custom)
        }
        candidates.append(contentsOf: [
            "\(home)/Library/Application Support/ModelHub/loom",
            "\(home)/.local/bin/loom",
            "/opt/homebrew/bin/loom",
            "/usr/local/bin/loom",
        ])
        return candidates.first { fm.isExecutableFile(atPath: $0) }
    }

    var hubKeyURL: URL {
        dataDir.appendingPathComponent("hub.key")
    }

    var healthURL: URL {
        URL(string: "http://127.0.0.1:\(gatewayPort)/health")!
    }

    var launchURL: URL {
        URL(string: "http://127.0.0.1:\(adminPort)/api/launch")!
    }

    func consoleURL(ticket: String, page: String?) -> URL {
        var fragment = "ticket=\(ticket)"
        if let page, !page.isEmpty {
            fragment += "&page=\(page)"
        }
        return URL(string: "http://127.0.0.1:\(adminPort)/#\(fragment)")!
    }
}

@MainActor
final class LoomDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate, WKNavigationDelegate, WKUIDelegate {
    private let config = LoomConfig()
    private let loopbackSession: URLSession = {
        let cfg = URLSessionConfiguration.ephemeral
        cfg.connectionProxyDictionary = [:]
        cfg.timeoutIntervalForRequest = 3.0
        cfg.timeoutIntervalForResource = 5.0
        return URLSession(configuration: cfg)
    }()

    private var statusItem: NSStatusItem?
    private var statusMenuHeader: NSMenuItem?
    private var window: NSWindow?
    private var webView: WKWebView?
    private var offlinePanel: NSView?
    private var offlineStatusLabel: NSTextField?
    private var startServiceButton: NSButton?
    private var healthTimer: Timer?
    private var isStartingService = false
    private var hasLoadedConsole = false

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        setupMainMenu()
        setupStatusBar()
        setupWindow()
        refreshGatewayStatus()
        openWorkspace(page: nil)
        healthTimer = Timer.scheduledTimer(withTimeInterval: 5.0, repeats: true) { [weak self] _ in
            Task { @MainActor in
                self?.refreshGatewayStatus()
            }
        }
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows flag: Bool) -> Bool {
        if !flag {
            openWorkspace(page: nil)
        }
        return true
    }

    func windowShouldClose(_ sender: NSWindow) -> Bool {
        sender.orderOut(nil)
        return false
    }

    private func setupMainMenu() {
        let mainMenu = NSMenu()

        let appMenuItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(withTitle: "打开 Loom 工作台", action: #selector(menuOpenWorkspace), keyEquivalent: "o")
        appMenu.addItem(withTitle: "配置模型厂商", action: #selector(menuOpenProviders), keyEquivalent: ",")
        appMenu.addItem(NSMenuItem.separator())
        appMenu.addItem(withTitle: "重新加载界面", action: #selector(menuReload), keyEquivalent: "r")
        appMenu.addItem(withTitle: "关闭窗口", action: #selector(NSWindow.performClose(_:)), keyEquivalent: "w")
        appMenu.addItem(NSMenuItem.separator())
        appMenu.addItem(withTitle: "退出 Loom", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        appMenuItem.submenu = appMenu
        mainMenu.addItem(appMenuItem)

        let editMenuItem = NSMenuItem()
        let editMenu = NSMenu(title: "编辑")
        editMenu.addItem(withTitle: "撤销", action: Selector(("undo:")), keyEquivalent: "z")
        editMenu.addItem(withTitle: "重做", action: Selector(("redo:")), keyEquivalent: "Z")
        editMenu.addItem(NSMenuItem.separator())
        editMenu.addItem(withTitle: "剪切", action: #selector(NSText.cut(_:)), keyEquivalent: "x")
        editMenu.addItem(withTitle: "复制", action: #selector(NSText.copy(_:)), keyEquivalent: "c")
        editMenu.addItem(withTitle: "粘贴", action: #selector(NSText.paste(_:)), keyEquivalent: "v")
        editMenu.addItem(withTitle: "全选", action: #selector(NSText.selectAll(_:)), keyEquivalent: "a")
        editMenuItem.submenu = editMenu
        mainMenu.addItem(editMenuItem)

        NSApp.mainMenu = mainMenu
    }

    private func setupStatusBar() {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)
        item.button?.title = "⌘"
        item.button?.toolTip = "Loom 本机模型网关"

        let menu = NSMenu()
        let header = NSMenuItem(title: "Loom 网关：检测中…", action: nil, keyEquivalent: "")
        header.isEnabled = false
        menu.addItem(header)
        statusMenuHeader = header

        menu.addItem(NSMenuItem.separator())
        menu.addItem(withTitle: "打开 Loom 工作台", action: #selector(menuOpenWorkspace), keyEquivalent: "")
        menu.addItem(withTitle: "配置模型厂商", action: #selector(menuOpenProviders), keyEquivalent: "")
        menu.addItem(withTitle: "新手引导向导", action: #selector(menuOpenOnboarding), keyEquivalent: "")
        menu.addItem(NSMenuItem.separator())
        menu.addItem(withTitle: "启动 / 重启后台网关", action: #selector(menuStartService), keyEquivalent: "")
        menu.addItem(withTitle: "停止后台网关", action: #selector(menuStopService), keyEquivalent: "")
        menu.addItem(withTitle: "同步本地编程客户端", action: #selector(menuSyncClients), keyEquivalent: "")
        menu.addItem(withTitle: "运行连通性自检", action: #selector(menuDoctor), keyEquivalent: "")
        menu.addItem(NSMenuItem.separator())
        menu.addItem(withTitle: "在默认浏览器打开", action: #selector(menuOpenInBrowser), keyEquivalent: "")
        menu.addItem(NSMenuItem.separator())
        menu.addItem(withTitle: "退出 Loom", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "")

        item.menu = menu
        statusItem = item
    }

    private func setupWindow() {
        let rect = NSRect(x: 0, y: 0, width: 1260, height: 820)
        let win = NSWindow(
            contentRect: rect,
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered,
            defer: false
        )
        win.title = "Loom · 本机模型网关工作台"
        win.minSize = NSSize(width: 860, height: 600)
        win.center()
        win.delegate = self

        let root = NSView(frame: rect)
        root.wantsLayer = true
        root.layer?.backgroundColor = NSColor.white.cgColor
        win.contentView = root

        let webConfig = WKWebViewConfiguration()
        webConfig.preferences.javaScriptCanOpenWindowsAutomatically = true
        let wv = WKWebView(frame: .zero, configuration: webConfig)
        wv.translatesAutoresizingMaskIntoConstraints = false
        wv.navigationDelegate = self
        wv.uiDelegate = self
        root.addSubview(wv)

        let offline = buildOfflinePanel()
        offline.translatesAutoresizingMaskIntoConstraints = false
        offline.isHidden = true
        root.addSubview(offline)

        NSLayoutConstraint.activate([
            wv.topAnchor.constraint(equalTo: root.topAnchor),
            wv.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            wv.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            wv.trailingAnchor.constraint(equalTo: root.trailingAnchor),

            offline.topAnchor.constraint(equalTo: root.topAnchor),
            offline.bottomAnchor.constraint(equalTo: root.bottomAnchor),
            offline.leadingAnchor.constraint(equalTo: root.leadingAnchor),
            offline.trailingAnchor.constraint(equalTo: root.trailingAnchor),
        ])

        self.window = win
        self.webView = wv
        self.offlinePanel = offline
    }

    private func buildOfflinePanel() -> NSView {
        let container = NSView()
        container.wantsLayer = true
        container.layer?.backgroundColor = NSColor(calibratedRed: 0.973, green: 0.980, blue: 0.973, alpha: 1.0).cgColor

        let card = NSView()
        card.translatesAutoresizingMaskIntoConstraints = false
        card.wantsLayer = true
        card.layer?.backgroundColor = NSColor.white.cgColor
        card.layer?.cornerRadius = 16
        card.layer?.borderWidth = 1
        card.layer?.borderColor = NSColor(calibratedRed: 0.89, green: 0.92, blue: 0.90, alpha: 1.0).cgColor
        container.addSubview(card)

        let brand = NSTextField(labelWithString: "⌘")
        brand.font = NSFont.systemFont(ofSize: 44, weight: .bold)
        brand.textColor = NSColor(calibratedRed: 0.082, green: 0.341, blue: 0.259, alpha: 1.0)
        brand.alignment = .center

        let title = NSTextField(labelWithString: "欢迎使用 Loom 本机模型工作台")
        title.font = NSFont.systemFont(ofSize: 22, weight: .bold)
        title.textColor = NSColor(calibratedRed: 0.082, green: 0.114, blue: 0.141, alpha: 1.0)
        title.alignment = .center

        let desc = NSTextField(wrappingLabelWithString: "Loom 仅在你的电脑本机（127.0.0.1）运行，统一管理模型 API Key 与 AI 编程工具配置，密钥永不上传外网。\n当前后台网关尚未启动，点击下方按钮即可一键启动并进入新手引导。")
        desc.font = NSFont.systemFont(ofSize: 13.5)
        desc.textColor = NSColor(calibratedRed: 0.42, green: 0.46, blue: 0.51, alpha: 1.0)
        desc.alignment = .center

        let status = NSTextField(wrappingLabelWithString: "端口检测：127.0.0.1:\(config.gatewayPort)")
        status.font = NSFont.monospacedSystemFont(ofSize: 12, weight: .regular)
        status.textColor = NSColor(calibratedRed: 0.42, green: 0.46, blue: 0.51, alpha: 1.0)
        status.alignment = .center
        self.offlineStatusLabel = status

        let startBtn = NSButton(title: "一键启动后台服务并进入", target: self, action: #selector(handleStartFromPanel))
        startBtn.bezelStyle = .rounded
        startBtn.isBordered = true
        startBtn.contentTintColor = NSColor(calibratedRed: 0.082, green: 0.341, blue: 0.259, alpha: 1.0)
        startBtn.font = NSFont.systemFont(ofSize: 14, weight: .semibold)
        self.startServiceButton = startBtn

        let retryBtn = NSButton(title: "重新检测", target: self, action: #selector(handleRetryFromPanel))
        retryBtn.bezelStyle = .rounded
        retryBtn.font = NSFont.systemFont(ofSize: 13)

        let btnRow = NSStackView(views: [startBtn, retryBtn])
        btnRow.orientation = .horizontal
        btnRow.spacing = 12
        btnRow.alignment = .centerY

        let stack = NSStackView(views: [brand, title, desc, status, btnRow])
        stack.translatesAutoresizingMaskIntoConstraints = false
        stack.orientation = .vertical
        stack.spacing = 14
        stack.alignment = .centerX
        card.addSubview(stack)

        NSLayoutConstraint.activate([
            card.centerXAnchor.constraint(equalTo: container.centerXAnchor),
            card.centerYAnchor.constraint(equalTo: container.centerYAnchor),
            card.widthAnchor.constraint(equalToConstant: 560),
            stack.topAnchor.constraint(equalTo: card.topAnchor, constant: 34),
            stack.bottomAnchor.constraint(equalTo: card.bottomAnchor, constant: -34),
            stack.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: 34),
            stack.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -34),
            desc.widthAnchor.constraint(equalToConstant: 460),
            startBtn.heightAnchor.constraint(equalToConstant: 36),
            retryBtn.heightAnchor.constraint(equalToConstant: 36),
        ])

        return container
    }

    @objc private func menuOpenWorkspace() {
        openWorkspace(page: nil)
    }

    @objc private func menuOpenProviders() {
        openWorkspace(page: "providers")
    }

    @objc private func menuOpenOnboarding() {
        openWorkspace(page: "onboarding")
    }

    @objc private func menuReload() {
        hasLoadedConsole = false
        openWorkspace(page: nil)
    }

    @objc private func menuStartService() {
        startServiceAndOpen(targetPage: nil)
    }

    @objc private func handleStartFromPanel() {
        startServiceAndOpen(targetPage: "onboarding")
    }

    @objc private func handleRetryFromPanel() {
        openWorkspace(page: nil)
    }

    @objc private func menuStopService() {
        let uid = getuid()
        runCommandAsync(program: "/bin/launchctl", arguments: ["bootout", "gui/\(uid)/\(serviceLabel)"]) { [weak self] _, _ in
            self?.hasLoadedConsole = false
            self?.refreshGatewayStatus()
            self?.showOfflinePanel(status: "后台网关已停止。可随时从菜单栏重新启动。")
        }
    }

    @objc private func menuSyncClients() {
        guard let bin = config.resolveBinary() else {
            showAlert(title: "未找到 Loom 命令", message: "请先在 Loom 代码目录运行 ./setup.sh 完成安装。")
            return
        }
        runCommandAsync(program: bin, arguments: ["--data-dir", config.dataDir.path, "sync"]) { [weak self] code, output in
            if code == 0 {
                self?.showAlert(title: "客户端同步完成", message: output.isEmpty ? "已同步 pi / opencode2 与已有 Claude Code 配置。" : output)
            } else {
                self?.showAlert(title: "同步未完成", message: output.isEmpty ? "请运行 loom doctor 查看客户端配置状态。" : output)
            }
        }
    }

    @objc private func menuDoctor() {
        guard let bin = config.resolveBinary() else {
            showAlert(title: "未找到 Loom 命令", message: "请先在 Loom 代码目录运行 ./setup.sh 完成安装。")
            return
        }
        runCommandAsync(program: bin, arguments: ["--data-dir", config.dataDir.path, "doctor", "--ping"]) { [weak self] _, output in
            self?.showAlert(title: "Loom 自检结果", message: output.isEmpty ? "诊断完成。" : output)
        }
    }

    @objc private func menuOpenInBrowser() {
        Task { @MainActor in
            guard let ticket = await fetchTicket() else {
                showAlert(title: "无法获取本机登录票据", message: "请先启动后台网关并确保 hub.key 已生成。")
                return
            }
            NSWorkspace.shared.open(config.consoleURL(ticket: ticket, page: nil))
        }
    }

    private func openWorkspace(page: String?) {
        window?.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
        Task { @MainActor in
            let healthy = await checkHealth()
            updateStatusMenu(healthy: healthy)
            if !healthy {
                hasLoadedConsole = false
                showOfflinePanel(status: "未检测到运行中的网关 (127.0.0.1:\(config.gatewayPort))")
                return
            }
            if hasLoadedConsole, let page, let wv = webView {
                offlinePanel?.isHidden = true
                wv.isHidden = false
                _ = try? await wv.evaluateJavaScript("window.location.hash = '#\(page)';")
                return
            }
            guard let ticket = await fetchTicket() else {
                showOfflinePanel(status: "网关已启动，但未能换取本机登录票据，请确认管控台端口 127.0.0.1:\(config.adminPort) 可用。")
                return
            }
            offlinePanel?.isHidden = true
            webView?.isHidden = false
            hasLoadedConsole = true
            webView?.load(URLRequest(url: config.consoleURL(ticket: ticket, page: page)))
        }
    }

    private func showOfflinePanel(status: String) {
        offlineStatusLabel?.stringValue = status
        webView?.isHidden = true
        offlinePanel?.isHidden = false
    }

    private func startServiceAndOpen(targetPage: String?) {
        if isStartingService {
            return
        }
        guard let bin = config.resolveBinary() else {
            showAlert(title: "未找到 Loom 可执行文件", message: "请先在 Loom 目录终端执行 ./setup.sh 完成编译与安装。")
            return
        }
        isStartingService = true
        startServiceButton?.isEnabled = false
        offlineStatusLabel?.stringValue = "正在启动本机后台服务并等待健康检查…"

        runCommandAsync(program: bin, arguments: ["--data-dir", config.dataDir.path, "install"]) { [weak self] code, output in
            guard let self else { return }
            self.isStartingService = false
            self.startServiceButton?.isEnabled = true
            if code != 0 {
                self.showOfflinePanel(status: "启动失败，请确认系统允许后台自启动项或运行 loom doctor。")
                if !output.isEmpty {
                    self.showAlert(title: "启动后台服务未成功", message: output)
                }
                return
            }
            self.hasLoadedConsole = false
            self.openWorkspace(page: targetPage)
        }
    }

    private func refreshGatewayStatus() {
        Task { @MainActor in
            let healthy = await checkHealth()
            updateStatusMenu(healthy: healthy)
        }
    }

    private func updateStatusMenu(healthy: Bool) {
        if healthy {
            statusMenuHeader?.title = "● 网关运行中 (127.0.0.1:\(config.gatewayPort))"
            statusItem?.button?.title = "⌘"
        } else {
            statusMenuHeader?.title = "○ 网关未运行"
            statusItem?.button?.title = "⌘·"
        }
    }

    private func checkHealth() async -> Bool {
        var req = URLRequest(url: config.healthURL)
        req.timeoutInterval = 1.2
        do {
            let (data, resp) = try await loopbackSession.data(for: req)
            guard let http = resp as? HTTPURLResponse, http.statusCode == 200 else {
                return false
            }
            if let obj = try JSONSerialization.jsonObject(with: data) as? [String: Any],
               let ok = obj["ok"] as? Bool,
               let runtime = obj["runtime"] as? String {
                return ok && runtime == "rust"
            }
            return false
        } catch {
            return false
        }
    }

    private func fetchTicket() async -> String? {
        guard let rawKey = try? String(contentsOf: config.hubKeyURL, encoding: .utf8) else {
            return nil
        }
        let key = rawKey.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !key.isEmpty else { return nil }

        var req = URLRequest(url: config.launchURL)
        req.httpMethod = "POST"
        req.setValue("Bearer \(key)", forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = Data("{}".utf8)
        do {
            let (data, resp) = try await loopbackSession.data(for: req)
            guard let http = resp as? HTTPURLResponse, http.statusCode == 200 else {
                return nil
            }
            if let obj = try JSONSerialization.jsonObject(with: data) as? [String: Any] {
                return obj["ticket"] as? String
            }
            return nil
        } catch {
            return nil
        }
    }

    private func runCommandAsync(program: String, arguments: [String], completion: @escaping @MainActor (Int32, String) -> Void) {
        let env = ProcessInfo.processInfo.environment
        DispatchQueue.global(qos: .userInitiated).async {
            let proc = Process()
            proc.executableURL = URL(fileURLWithPath: program)
            proc.arguments = arguments
            proc.environment = env
            proc.standardInput = FileHandle.nullDevice
            let pipe = Pipe()
            proc.standardOutput = pipe
            proc.standardError = pipe
            do {
                try proc.run()
                proc.waitUntilExit()
                let data = pipe.fileHandleForReading.readDataToEndOfFile()
                let text = String(data: data, encoding: .utf8)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
                let status = proc.terminationStatus
                DispatchQueue.main.async {
                    completion(status, text)
                }
            } catch {
                DispatchQueue.main.async {
                    completion(-1, error.localizedDescription)
                }
            }
        }
    }

    private func showAlert(title: String, message: String) {
        let alert = NSAlert()
        alert.messageText = title
        alert.informativeText = message
        alert.addButton(withTitle: "好的")
        if let win = window, win.isVisible {
            alert.beginSheetModal(for: win)
        } else {
            alert.runModal()
        }
    }

    // MARK: - WKNavigationDelegate

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        hasLoadedConsole = false
        showOfflinePanel(status: "本机管控台暂未就绪 (127.0.0.1:\(config.adminPort))，请点击重新检测或重启后台服务。")
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        hasLoadedConsole = false
        showOfflinePanel(status: "页面加载中断，请点击重新检测。")
    }

    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction, decisionHandler: @escaping @MainActor @Sendable (WKNavigationActionPolicy) -> Void) {
        guard let url = navigationAction.request.url else {
            decisionHandler(.allow)
            return
        }
        let host = url.host?.lowercased() ?? ""
        if ["127.0.0.1", "localhost", "loom.localhost"].contains(host) {
            decisionHandler(.allow)
        } else {
            NSWorkspace.shared.open(url)
            decisionHandler(.cancel)
        }
    }

    // MARK: - WKUIDelegate

    func webView(_ webView: WKWebView, createWebViewWith configuration: WKWebViewConfiguration, for navigationAction: WKNavigationAction, windowFeatures: WKWindowFeatures) -> WKWebView? {
        if let url = navigationAction.request.url {
            NSWorkspace.shared.open(url)
        }
        return nil
    }

    func webView(_ webView: WKWebView, runJavaScriptAlertPanelWithMessage message: String, initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping @MainActor @Sendable () -> Void) {
        let alert = NSAlert()
        alert.messageText = "Loom 提示"
        alert.informativeText = message
        alert.addButton(withTitle: "确定")
        if let win = window {
            alert.beginSheetModal(for: win) { _ in completionHandler() }
        } else {
            alert.runModal()
            completionHandler()
        }
    }

    func webView(_ webView: WKWebView, runJavaScriptConfirmPanelWithMessage message: String, initiatedByFrame frame: WKFrameInfo, completionHandler: @escaping @MainActor @Sendable (Bool) -> Void) {
        let alert = NSAlert()
        alert.messageText = "请确认操作"
        alert.informativeText = message
        alert.addButton(withTitle: "确定")
        alert.addButton(withTitle: "取消")
        if let win = window {
            alert.beginSheetModal(for: win) { resp in
                completionHandler(resp == .alertFirstButtonReturn)
            }
        } else {
            completionHandler(alert.runModal() == .alertFirstButtonReturn)
        }
    }
}

MainActor.assumeIsolated {
    let app = NSApplication.shared
    let delegate = LoomDelegate()
    app.delegate = delegate
    app.run()
}
