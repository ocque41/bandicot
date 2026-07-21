import Foundation

public let protocolVersion = 1
public let maximumFrameBytes = 8 * 1024 * 1024
public let maximumErrorCharacters = 4096

public struct BridgeRequest: Codable, Sendable {
    public let protocolVersion: Int
    public let operation: String
    public let model: String
    public let messages: [BridgeMessage]
    public let temperature: Float?
    public let maximumResponseTokens: UInt32?
    public let jsonSchema: JSONValue?

    enum CodingKeys: String, CodingKey {
        case protocolVersion = "protocol_version"
        case operation
        case model
        case messages
        case temperature
        case maximumResponseTokens = "maximum_response_tokens"
        case jsonSchema = "json_schema"
    }
}

public struct BridgeMessage: Codable, Sendable {
    public let role: String
    public let content: String
    public let toolCalls: [BridgeToolCall]?
    public let toolCallID: String?

    enum CodingKeys: String, CodingKey {
        case role
        case content
        case toolCalls = "tool_calls"
        case toolCallID = "tool_call_id"
    }
}

public struct BridgeToolCall: Codable, Sendable {
    public let id: String
    public let name: String
    public let arguments: String
}

public enum JSONValue: Codable, Sendable {
    case object([String: JSONValue])
    case array([JSONValue])
    case string(String)
    case number(Double)
    case bool(Bool)
    case null

    public init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() { self = .null }
        else if let value = try? container.decode(Bool.self) { self = .bool(value) }
        else if let value = try? container.decode(Double.self) { self = .number(value) }
        else if let value = try? container.decode(String.self) { self = .string(value) }
        else if let value = try? container.decode([JSONValue].self) { self = .array(value) }
        else { self = .object(try container.decode([String: JSONValue].self)) }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.singleValueContainer()
        switch self {
        case .object(let value): try container.encode(value)
        case .array(let value): try container.encode(value)
        case .string(let value): try container.encode(value)
        case .number(let value): try container.encode(value)
        case .bool(let value): try container.encode(value)
        case .null: try container.encodeNil()
        }
    }

    public var objectValue: [String: JSONValue]? {
        guard case .object(let value) = self else { return nil }
        return value
    }

    public var stringValue: String? {
        guard case .string(let value) = self else { return nil }
        return value
    }

    public var arrayValue: [JSONValue]? {
        guard case .array(let value) = self else { return nil }
        return value
    }
}

public enum BridgeFrame: Codable, Sendable, Equatable {
    case available
    case snapshot(text: String)
    case completed(text: String)
    case error(code: String, message: String)

    enum CodingKeys: String, CodingKey { case type, text, code, message }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(String.self, forKey: .type) {
        case "available": self = .available
        case "snapshot": self = .snapshot(text: try container.decode(String.self, forKey: .text))
        case "completed": self = .completed(text: try container.decode(String.self, forKey: .text))
        case "error": self = .error(
            code: try container.decode(String.self, forKey: .code),
            message: try container.decode(String.self, forKey: .message)
        )
        default: throw ProtocolError.invalidFrame
        }
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .available:
            try container.encode("available", forKey: .type)
        case .snapshot(let text):
            try container.encode("snapshot", forKey: .type)
            try container.encode(text, forKey: .text)
        case .completed(let text):
            try container.encode("completed", forKey: .type)
            try container.encode(text, forKey: .text)
        case .error(let code, let message):
            try container.encode("error", forKey: .type)
            try container.encode(code, forKey: .code)
            try container.encode(message, forKey: .message)
        }
    }
}

public enum ProtocolError: Error {
    case unexpectedEOF
    case invalidFrame
    case frameTooLarge
}

public func bounded(_ value: String) -> String {
    String(value.prefix(maximumErrorCharacters))
}

public func readFrame<T: Decodable>(_ type: T.Type, from handle: FileHandle) throws -> T? {
    guard let header = try readExactly(4, from: handle) else { return nil }
    let length = header.reduce(0) { ($0 << 8) | Int($1) }
    guard length > 0, length <= maximumFrameBytes else { throw ProtocolError.frameTooLarge }
    guard let payload = try readExactly(length, from: handle) else { throw ProtocolError.unexpectedEOF }
    return try JSONDecoder().decode(type, from: payload)
}

public func writeFrame<T: Encodable>(_ value: T, to handle: FileHandle) throws {
    let payload = try JSONEncoder().encode(value)
    guard !payload.isEmpty, payload.count <= maximumFrameBytes else { throw ProtocolError.frameTooLarge }
    let length = UInt32(payload.count).bigEndian
    try withUnsafeBytes(of: length) { try handle.write(contentsOf: Data($0)) }
    try handle.write(contentsOf: payload)
}

private func readExactly(_ count: Int, from handle: FileHandle) throws -> Data? {
    var data = Data()
    while data.count < count {
        guard let chunk = try handle.read(upToCount: count - data.count), !chunk.isEmpty else {
            if data.isEmpty { return nil }
            throw ProtocolError.unexpectedEOF
        }
        data.append(chunk)
    }
    return data
}
