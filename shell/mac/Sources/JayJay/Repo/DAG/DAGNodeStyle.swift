import JayJayCore
import SwiftUI

struct DAGNodeStyle {
    enum Shape: Equatable {
        case circle
        case diamond
    }

    enum Fill {
        case filled(Color)
        case outlined(Color, lineWidth: CGFloat)
    }

    private static let diamondRadiusScale = (CGFloat.pi / 2).squareRoot()

    let shape: Shape
    let radius: CGFloat
    let fill: Fill

    func path(in rect: CGRect) -> Path {
        switch shape {
            case .circle:
                Path(ellipseIn: rect)
            case .diamond:
                Path { p in
                    let cx = rect.midX
                    let cy = rect.midY
                    p.move(to: CGPoint(x: cx, y: rect.minY))
                    p.addLine(to: CGPoint(x: rect.maxX, y: cy))
                    p.addLine(to: CGPoint(x: cx, y: rect.maxY))
                    p.addLine(to: CGPoint(x: rect.minX, y: cy))
                    p.closeSubpath()
                }
        }
    }

    static func resolve(change: ChangeInfo, radius baseRadius: CGFloat = DAGGeometry.preferredNodeRadius) -> DAGNodeStyle {
        DAGNodeStyle(
            isWorkingCopy: change.isWorkingCopy,
            isImmutable: change.isImmutable,
            hasConflict: change.hasConflict,
            isEmpty: change.isEmpty,
            hasBookmark: !change.bookmarks.isEmpty,
            baseRadius: baseRadius
        )
    }

    init(
        isWorkingCopy: Bool,
        isImmutable: Bool,
        hasConflict: Bool,
        isEmpty: Bool,
        hasBookmark: Bool,
        baseRadius: CGFloat = DAGGeometry.preferredNodeRadius
    ) {
        shape = isImmutable ? .diamond : .circle
        radius = Self.radius(for: shape, hasBookmark: hasBookmark, baseRadius: baseRadius)
        fill = if isWorkingCopy {
            .filled(.accentColor)
        } else if hasConflict {
            .filled(.red)
        } else if isEmpty {
            .outlined(.secondary, lineWidth: 1.5)
        } else if hasBookmark {
            .outlined(.accentColor, lineWidth: 1.8)
        } else {
            .filled(.secondary)
        }
    }

    static func radius(for shape: Shape, hasBookmark: Bool, baseRadius: CGFloat = DAGGeometry.preferredNodeRadius) -> CGFloat {
        let circleRadius: CGFloat = hasBookmark ? baseRadius + 1 : baseRadius
        return shape == .diamond ? circleRadius * diamondRadiusScale : circleRadius
    }

    func draw(_ path: Path, in ctx: GraphicsContext) {
        switch fill {
            case let .filled(color):
                ctx.fill(path, with: .color(color))
            case let .outlined(color, lineWidth):
                ctx.stroke(path, with: .color(color), style: StrokeStyle(lineWidth: lineWidth))
        }
    }
}
