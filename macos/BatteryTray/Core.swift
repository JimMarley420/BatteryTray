import Foundation

/// The Rust core: a command goes in as JSON, the answer comes back as JSON. Only ever called on the main thread.
enum Core {
    /// Runs a command and returns its answer as raw JSON.
    static func data(_ command: [String: Any]) -> Data {
        guard let json = try? JSONSerialization.data(withJSONObject: command),
              let text = String(data: json, encoding: .utf8),
              let answer = text.withCString({ bt_call($0) })
        else { return Data("{}".utf8) }
        defer { bt_free(answer) }
        return Data(bytes: answer, count: strlen(answer))
    }

    /// Runs a command and returns its answer as a dictionary.
    @discardableResult
    static func call(_ command: [String: Any]) -> [String: Any] {
        (try? JSONSerialization.jsonObject(with: data(command))) as? [String: Any] ?? [:]
    }

    /// A line in the app's log (log.txt), next to the core's.
    static func log(_ text: String) {
        call(["cmd": "log", "text": text])
    }

    /// Runs a command and decodes its answer.
    static func decode<T: Decodable>(_ type: T.Type, _ command: [String: Any]) -> T? {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return try? decoder.decode(type, from: data(command))
    }
}
