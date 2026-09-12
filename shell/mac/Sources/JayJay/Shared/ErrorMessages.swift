import Foundation
import JayJayCore

extension Error {
    var friendlyDescription: String {
        if let jjError = self as? JayJayError {
            switch jjError {
                case let .RepoNotFound(path):
                    return "No Jujutsu repository found at \(URL(fileURLWithPath: path).lastPathComponent)"
                case let .RevNotFound(rev):
                    return "Revision not found: \(rev)"
                case let .Review(message):
                    return unwrapCommandError(message: message)
                case let .Diff(message):
                    return unwrapCommandError(message: message)
                case let .DiffSelectionStale(path):
                    return "\(path): file changed since the diff was rendered — refresh and retry"
                case let .ConflictEditorStale(path):
                    return "\(path): conflict changed since the editor opened — refresh and retry"
                case let .FileEditorStale(path):
                    return "\(path): file changed since the editor opened — refresh and retry"
                case let .Internal(message):
                    return unwrapCommandError(message: message)
                case .Canceled:
                    return "Canceled"
            }
        }
        return localizedDescription
    }
}
