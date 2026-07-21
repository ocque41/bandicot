import BandicotFoundationModelsBridgeCore
import Foundation
import FoundationModels

@main
struct BridgeMain {
    static func main() async {
        do {
            guard let request = try readFrame(BridgeRequest.self, from: .standardInput) else {
                throw BridgeFailure(code: "missing_request", message: "No framed request was received")
            }
            guard request.protocolVersion == protocolVersion else {
                throw BridgeFailure(code: "protocol_version", message: "Unsupported bridge protocol version")
            }
            guard request.operation == "generate" || request.operation == "availability" else {
                throw BridgeFailure(code: "invalid_operation", message: "Unsupported bridge operation")
            }
            guard #available(macOS 26.0, *) else {
                throw BridgeFailure(code: "unsupported_os", message: "Apple Foundation Models requires macOS 26 or later")
            }
            try await run(request)
        } catch let failure as BridgeFailure {
            try? writeFrame(
                BridgeFrame.error(code: bounded(failure.code), message: bounded(failure.message)),
                to: .standardOutput
            )
        } catch {
            try? writeFrame(
                BridgeFrame.error(code: "bridge_error", message: bounded(error.localizedDescription)),
                to: .standardOutput
            )
        }
    }

    @available(macOS 26.0, *)
    private static func run(_ request: BridgeRequest) async throws {
        let model = SystemLanguageModel.default
        switch model.availability {
        case .available:
            try writeFrame(BridgeFrame.available, to: .standardOutput)
        case .unavailable(let reason):
            throw BridgeFailure(code: "model_unavailable", message: unavailableMessage(reason))
        }
        if request.operation == "availability" {
            try writeFrame(BridgeFrame.completed(text: ""), to: .standardOutput)
            return
        }

        let mapped = try mapConversation(request.messages)
        let session = LanguageModelSession(model: model, transcript: Transcript(entries: mapped.entries))
        let options = GenerationOptions(
            temperature: request.temperature.map(Double.init),
            maximumResponseTokens: request.maximumResponseTokens.map(Int.init)
        )
        do {
            if let jsonSchema = request.jsonSchema {
                let schema = try generationSchema(from: jsonSchema)
                let response = try await session.respond(
                    to: mapped.prompt,
                    schema: schema,
                    options: options
                )
                try writeFrame(BridgeFrame.completed(text: response.content.jsonString), to: .standardOutput)
            } else {
                let stream = session.streamResponse(to: mapped.prompt, options: options)
                var finalText = ""
                for try await snapshot in stream {
                    finalText = snapshot.content
                    try writeFrame(BridgeFrame.snapshot(text: finalText), to: .standardOutput)
                }
                try writeFrame(BridgeFrame.completed(text: finalText), to: .standardOutput)
            }
        } catch let error as LanguageModelSession.GenerationError {
            throw mapGenerationError(error)
        }
    }
}

private struct BridgeFailure: Error {
    let code: String
    let message: String
}

@available(macOS 26.0, *)
private func unavailableMessage(_ reason: SystemLanguageModel.Availability.UnavailableReason) -> String {
    switch reason {
    case .deviceNotEligible: "This Mac is not eligible for Apple Intelligence"
    case .appleIntelligenceNotEnabled: "Apple Intelligence is not enabled"
    case .modelNotReady: "The on-device language model is not ready"
    @unknown default: "The on-device language model is unavailable"
    }
}

@available(macOS 26.0, *)
private func mapGenerationError(_ error: LanguageModelSession.GenerationError) -> BridgeFailure {
    let code: String
    switch error {
    case .exceededContextWindowSize: code = "context_window_exceeded"
    case .assetsUnavailable: code = "assets_unavailable"
    case .guardrailViolation: code = "guardrail_violation"
    case .unsupportedGuide: code = "unsupported_schema"
    case .unsupportedLanguageOrLocale: code = "unsupported_locale"
    case .decodingFailure: code = "decoding_failure"
    case .rateLimited: code = "rate_limited"
    case .concurrentRequests: code = "concurrent_requests"
    case .refusal: code = "refusal"
    @unknown default: code = "generation_error"
    }
    return BridgeFailure(code: code, message: bounded(error.localizedDescription))
}

@available(macOS 26.0, *)
private func mapConversation(_ messages: [BridgeMessage]) throws -> (entries: [Transcript.Entry], prompt: String) {
    guard let promptIndex = messages.lastIndex(where: { $0.role == "user" }) else {
        throw BridgeFailure(code: "invalid_conversation", message: "A user message is required")
    }
    var entries: [Transcript.Entry] = []
    var toolNames: [String: String] = [:]
    for (index, message) in messages.enumerated() where index != promptIndex {
        let segment = Transcript.Segment.text(.init(content: message.content))
        switch message.role {
        case "system":
            entries.append(.instructions(.init(segments: [segment], toolDefinitions: [])))
        case "user":
            entries.append(.prompt(.init(segments: [segment])))
        case "assistant":
            if !message.content.isEmpty {
                entries.append(.response(.init(assetIDs: [], segments: [segment])))
            }
            if let calls = message.toolCalls, !calls.isEmpty {
                let mapped = try calls.map { call in
                    toolNames[call.id] = call.name
                    return Transcript.ToolCall(
                        id: call.id,
                        toolName: call.name,
                        arguments: try GeneratedContent(json: call.arguments)
                    )
                }
                entries.append(.toolCalls(.init(mapped)))
            }
        case "tool_result":
            guard let id = message.toolCallID, let name = toolNames[id] else {
                throw BridgeFailure(code: "invalid_conversation", message: "Tool result has no matching tool call")
            }
            entries.append(.toolOutput(.init(id: id, toolName: name, segments: [segment])))
        default:
            throw BridgeFailure(code: "invalid_conversation", message: "Unknown conversation role")
        }
    }
    return (entries, messages[promptIndex].content)
}

@available(macOS 26.0, *)
private func generationSchema(from value: JSONValue) throws -> GenerationSchema {
    let root = try dynamicSchema(from: value, name: "BandicotResponse")
    return try GenerationSchema(root: root, dependencies: [])
}

@available(macOS 26.0, *)
private func dynamicSchema(from value: JSONValue, name: String) throws -> DynamicGenerationSchema {
    guard let object = value.objectValue else {
        throw BridgeFailure(code: "unsupported_schema", message: "JSON Schema nodes must be objects")
    }
    if let choices = object["enum"]?.arrayValue {
        let strings = choices.compactMap(\.stringValue)
        guard strings.count == choices.count, !strings.isEmpty else {
            throw BridgeFailure(code: "unsupported_schema", message: "Only non-empty string enums are supported")
        }
        return DynamicGenerationSchema(name: name, anyOf: strings)
    }
    if let alternatives = object["anyOf"]?.arrayValue {
        return DynamicGenerationSchema(
            name: name,
            anyOf: try alternatives.enumerated().map { try dynamicSchema(from: $0.element, name: "\(name)Choice\($0.offset)") }
        )
    }
    guard let type = object["type"]?.stringValue else {
        throw BridgeFailure(code: "unsupported_schema", message: "Every JSON Schema node needs a type")
    }
    switch type {
    case "object":
        let properties = object["properties"]?.objectValue ?? [:]
        let required = Set(object["required"]?.arrayValue?.compactMap(\.stringValue) ?? [])
        let mapped = try properties.keys.sorted().map { propertyName in
            DynamicGenerationSchema.Property(
                name: propertyName,
                description: properties[propertyName]?.objectValue?["description"]?.stringValue,
                schema: try dynamicSchema(from: properties[propertyName]!, name: "\(name)_\(propertyName)"),
                isOptional: !required.contains(propertyName)
            )
        }
        return DynamicGenerationSchema(name: name, description: object["description"]?.stringValue, properties: mapped)
    case "array":
        guard let items = object["items"] else {
            throw BridgeFailure(code: "unsupported_schema", message: "Array schemas need items")
        }
        return DynamicGenerationSchema(arrayOf: try dynamicSchema(from: items, name: "\(name)Item"))
    case "string": return DynamicGenerationSchema(type: String.self)
    case "integer": return DynamicGenerationSchema(type: Int.self)
    case "number": return DynamicGenerationSchema(type: Double.self)
    case "boolean": return DynamicGenerationSchema(type: Bool.self)
    case "null":
        if #available(macOS 26.4, *) { return .null }
        throw BridgeFailure(code: "unsupported_schema", message: "Null schemas require macOS 26.4 or later")
    default:
        throw BridgeFailure(code: "unsupported_schema", message: "Unsupported JSON Schema type")
    }
}
