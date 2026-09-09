import UIKit
import OSLog

@main
@MainActor
final class AppDelegate: UIResponder, UIApplicationDelegate {
    var window: UIWindow?

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        let window = UIWindow(frame: UIScreen.main.bounds)
        window.rootViewController = DemoViewController()
        window.makeKeyAndVisible()
        self.window = window
        return true
    }
}

@MainActor
final class DemoViewController: UIViewController {
    private let status = UILabel()
    private let name = UITextField()
    private var count = 0
    private let logger = Logger(subsystem: "dev.mx.demo", category: "interaction")

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemBackground

        let title = UILabel()
        title.text = "Mx Demo"
        title.font = .preferredFont(forTextStyle: .largeTitle)
        title.accessibilityTraits = .header

        status.text = "Count: 0"
        status.accessibilityIdentifier = "counter"
        status.font = .preferredFont(forTextStyle: .title2)

        let increment = UIButton(type: .system)
        increment.setTitle("Increment", for: .normal)
        increment.accessibilityIdentifier = "increment"
        increment.addTarget(self, action: #selector(incrementCounter), for: .touchUpInside)

        name.placeholder = "Your name"
        name.accessibilityLabel = "Your name"
        name.accessibilityIdentifier = "name"
        name.borderStyle = .roundedRect
        name.autocorrectionType = .no
        name.autocapitalizationType = .none
        name.smartQuotesType = .no
        name.smartDashesType = .no
        name.smartInsertDeleteType = .no

        let greet = UIButton(type: .system)
        greet.setTitle("Greet", for: .normal)
        greet.accessibilityIdentifier = "greet"
        greet.addTarget(self, action: #selector(greetUser), for: .touchUpInside)

        let stack = UIStackView(arrangedSubviews: [title, status, increment, name, greet])
        stack.axis = .vertical
        stack.spacing = 20
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 40),
            stack.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 24),
            stack.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -24),
            name.heightAnchor.constraint(greaterThanOrEqualToConstant: 44)
        ])
        logger.notice("Mx demo launched")
    }

    @objc private func incrementCounter() {
        count += 1
        status.text = "Count: \(count)"
        logger.notice("Counter incremented to \(self.count)")
    }

    @objc private func greetUser() {
        status.text = "Hello, \(name.text ?? "")!"
        name.resignFirstResponder()
        logger.notice("Greeting displayed")
    }
}
