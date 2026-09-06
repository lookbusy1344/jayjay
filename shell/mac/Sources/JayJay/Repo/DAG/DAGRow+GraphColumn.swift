import JayJayCore
import SwiftUI

enum DAGNodeLayerContent: Equatable {
    case nodeOnly
    case nodeAndOverflow
    case overflowOnly

    var includesNode: Bool {
        self != .overflowOnly
    }

    var includesOverflow: Bool {
        self != .nodeOnly
    }
}

func dagNodeLayerContent(
    isGraphClipped: Bool,
    nodeTrailingX: CGFloat,
    overflowLeadingX: CGFloat
) -> DAGNodeLayerContent {
    guard isGraphClipped else { return .nodeOnly }
    return nodeTrailingX + dagOverflowNodeGap < overflowLeadingX ? .nodeAndOverflow : .overflowOnly
}

extension DAGRow {
    var graphColumn: some View {
        let geometry = viewModel.geometry
        let row = viewModel.row
        let nodeColumn = Int(row?.nodeColumn ?? 0)
        let myX = geometry.xPosition(forColumn: nodeColumn)
        let nodeY = dagNodeCenterY
        let nodeStyle = DAGNodeStyle.resolve(change: change, radius: geometry.nodeRadius)

        return GeometryReader { geo in
            let bands = row?.elisionsAfter ?? []
            let height = geo.size.height
            let mainRowBottomY = dagMainRowBottomY(totalHeight: height, bandCount: bands.count)

            Canvas { ctx, _ in
                let lineColor = Color.secondary.opacity(0.2)
                let edgeColor = Color.secondary.opacity(0.3)

                let linkLine = row?.linkLine
                let linkCenterY = linkLine == nil ? nodeY : nodeY + (mainRowBottomY - nodeY) * dagLinkCenterFraction
                let linkBottomY = linkLine == nil ? nodeY : min(mainRowBottomY, linkCenterY + dagGraphCornerRadius)

                // The node line is the renderer state above this row's transition band.
                if let nodeLine = row?.nodeLine {
                    for (column, cell) in nodeLine.enumerated() where column != nodeColumn {
                        guard let style = strokeStyle(for: cell) else { continue }
                        let laneX = geometry.xPosition(forColumn: column)
                        let path = Path { p in
                            p.move(to: CGPoint(x: laneX, y: 0))
                            p.addLine(to: CGPoint(x: laneX, y: nodeY))
                        }
                        ctx.stroke(path, with: .color(lineColor), style: style)
                    }
                }

                if let incoming = row?.incoming {
                    let path = Path { p in
                        p.move(to: CGPoint(x: myX, y: 0))
                        p.addLine(to: CGPoint(x: myX, y: nodeY - nodeStyle.radius))
                    }
                    ctx.stroke(path, with: .color(lineColor), style: strokeStyle(for: incoming))
                }

                if let linkLine {
                    for (column, cell) in linkLine.enumerated() {
                        let x = geometry.xPosition(forColumn: column)
                        for component in cell.components {
                            let path = component.path(in: .init(
                                x: x,
                                topY: geometry.linkTopY(
                                    forColumn: column,
                                    nodeColumn: nodeColumn,
                                    nodeY: nodeY,
                                    nodeRadius: nodeStyle.radius
                                ),
                                centerY: linkCenterY,
                                bottomY: linkBottomY,
                                halfPitch: geometry.lanePitch / 2,
                                cornerRadius: dagGraphCornerRadius
                            ))
                            ctx.stroke(path, with: .color(edgeColor), style: strokeStyle(for: component.edgeKind))
                        }
                    }
                }

                // The pad line is the renderer state below the transition band.
                if let padLine = row?.padLine {
                    for (column, cell) in padLine.enumerated() {
                        guard let style = strokeStyle(for: cell) else { continue }
                        let x = geometry.xPosition(forColumn: column)
                        let startY = if linkLine != nil {
                            linkBottomY
                        } else if column == nodeColumn {
                            nodeY + nodeStyle.radius
                        } else {
                            nodeY
                        }
                        let path = Path { p in
                            p.move(to: CGPoint(x: x, y: startY))
                            p.addLine(to: CGPoint(x: x, y: mainRowBottomY))
                        }
                        ctx.stroke(path, with: .color(lineColor), style: style)
                    }
                }

                for column in row?.terminationColumns ?? [] {
                    let terminationX = geometry.xPosition(forColumn: Int(column))
                    let startY = if linkLine != nil {
                        linkBottomY
                    } else if Int(column) == nodeColumn {
                        nodeY + nodeStyle.radius
                    } else {
                        nodeY
                    }
                    let endY = startY + (mainRowBottomY - startY) * dagTerminationStubFraction
                    let path = Path { p in
                        p.move(to: CGPoint(x: terminationX, y: startY))
                        p.addLine(to: CGPoint(x: terminationX, y: endY))
                    }
                    ctx.stroke(path, with: .color(edgeColor), style: dagMissingEdgeStroke)
                    let capRect = CGRect(x: terminationX - 1.5, y: endY - 1.5, width: 3, height: 3)
                    ctx.fill(Path(ellipseIn: capRect), with: .color(edgeColor))
                }

                // Synthetic elision bands, drawn in the owning row's own Canvas so their lanes
                // connect seamlessly to the row above rather than resetting per band.
                var bandTopY = mainRowBottomY
                for band in bands {
                    let bandNodeColumn = Int(band.nodeColumn)
                    let bandBottomY = bandTopY + dagElisionBandHeight
                    let labelY = bandTopY + dagElisionBandHeight / 2
                    let bandX = geometry.xPosition(forColumn: bandNodeColumn)

                    for (column, cell) in band.nodeLine.enumerated() where column != bandNodeColumn {
                        guard let style = strokeStyle(for: cell) else { continue }
                        let laneX = geometry.xPosition(forColumn: column)
                        let path = Path { p in
                            p.move(to: CGPoint(x: laneX, y: bandTopY))
                            p.addLine(to: CGPoint(x: laneX, y: labelY))
                        }
                        ctx.stroke(path, with: .color(lineColor), style: style)
                    }
                    let incomingPath = Path { p in
                        p.move(to: CGPoint(x: bandX, y: bandTopY))
                        p.addLine(to: CGPoint(x: bandX, y: labelY))
                    }
                    ctx.stroke(incomingPath, with: .color(lineColor), style: dagSolidStroke)

                    if let bandLinkLine = band.linkLine {
                        for (column, cell) in bandLinkLine.enumerated() {
                            let x = geometry.xPosition(forColumn: column)
                            for component in cell.components {
                                let path = component.path(in: .init(
                                    x: x,
                                    topY: labelY,
                                    centerY: labelY,
                                    bottomY: labelY,
                                    halfPitch: geometry.lanePitch / 2,
                                    cornerRadius: dagGraphCornerRadius
                                ))
                                ctx.stroke(path, with: .color(edgeColor), style: strokeStyle(for: component.edgeKind))
                            }
                        }
                    }

                    let glyphRect = CGRect(x: bandX - 2, y: labelY - 2, width: 4, height: 4)
                    ctx.fill(Path(ellipseIn: glyphRect), with: .color(.secondary.opacity(0.55)))

                    for (column, cell) in band.padLine.enumerated() {
                        guard let style = strokeStyle(for: cell) else { continue }
                        let x = geometry.xPosition(forColumn: column)
                        let path = Path { p in
                            p.move(to: CGPoint(x: x, y: labelY))
                            p.addLine(to: CGPoint(x: x, y: bandBottomY))
                        }
                        ctx.stroke(path, with: .color(lineColor), style: style)
                    }

                    bandTopY = bandBottomY
                }
            }
            // The row's native lanes run past the gutter budget. They stay in the layout; only the
            // leading budget is painted, dissolving into the backdrop rather than ending on a wall.
            .mask {
                if viewModel.isGraphClipped {
                    overflowFadeMask(width: geo.size.width)
                } else {
                    Rectangle()
                }
            }
            // The node and overflow marker sit above the fade; when they would collide, the marker
            // represents the off-screen node instead of relocating that node into a visible lane.
            .overlay {
                nodeLayer(myX: myX, nodeY: nodeY, nodeStyle: nodeStyle, width: geo.size.width)
            }
            .clipped()
        }
    }

    private func nodeLayer(myX: CGFloat, nodeY: CGFloat, nodeStyle: DAGNodeStyle, width: CGFloat) -> some View {
        Canvas { ctx, _ in
            let arm = dagOverflowMarkerSize / 2
            // Anchor the badge by its disc so the whole circle clears the column edge, then place
            // the chevron centred in that disc.
            let haloCenterX = width - dagOverflowMarkerInset - dagOverflowMarkerHaloRadius
            let tipX = haloCenterX + (arm + dagOverflowChevronGap) / 2
            let overflowLeadingX = tipX - arm - dagOverflowChevronGap
            let content = dagNodeLayerContent(
                isGraphClipped: viewModel.isGraphClipped,
                nodeTrailingX: myX + nodeStyle.radius,
                overflowLeadingX: overflowLeadingX
            )
            let nodeRect = CGRect(
                x: myX - nodeStyle.radius,
                y: nodeY - nodeStyle.radius,
                width: nodeStyle.radius * 2,
                height: nodeStyle.radius * 2
            )
            let nodePath = nodeStyle.path(in: nodeRect)
            if content.includesNode {
                switch nodeStyle.fill {
                    case let .filled(color):
                        ctx.fill(nodePath, with: .color(color))
                    case let .outlined(color, lineWidth):
                        ctx.stroke(nodePath, with: .color(color), style: StrokeStyle(lineWidth: lineWidth))
                }

                if viewModel.isRebaseCandidate {
                    ctx.stroke(
                        nodePath,
                        with: .color(.accentColor.opacity(viewModel.isRebaseHoverTarget ? 1 : 0.55)),
                        style: StrokeStyle(lineWidth: viewModel.isRebaseHoverTarget ? 2.5 : 1.4)
                    )
                    if viewModel.isRebaseHoverTarget {
                        let ringRect = nodeRect.insetBy(dx: -4, dy: -4)
                        ctx.stroke(
                            nodeStyle.path(in: ringRect),
                            with: .color(.accentColor.opacity(0.45)),
                            style: StrokeStyle(lineWidth: 2)
                        )
                    }
                } else if viewModel.isRebaseSource {
                    ctx.stroke(
                        nodePath,
                        with: .color(.accentColor.opacity(0.75)),
                        style: StrokeStyle(lineWidth: 2)
                    )
                    if viewModel.isRebaseArmed {
                        let ringRect = nodeRect.insetBy(dx: -3, dy: -3)
                        ctx.stroke(
                            nodeStyle.path(in: ringRect),
                            with: .color(.accentColor.opacity(0.35)),
                            style: StrokeStyle(lineWidth: 1.5, dash: [3, 3])
                        )
                    }
                }
            }

            guard content.includesOverflow else { return }
            let markerColor = AppColors.dagOverflowMarker(viewModel.colorScheme)
            let haloCenter = CGPoint(x: haloCenterX, y: nodeY)
            let haloRect = CGRect(
                x: haloCenter.x - dagOverflowMarkerHaloRadius,
                y: haloCenter.y - dagOverflowMarkerHaloRadius,
                width: dagOverflowMarkerHaloRadius * 2,
                height: dagOverflowMarkerHaloRadius * 2
            )
            let haloPath = Path(ellipseIn: haloRect)
            // Opaque backdrop fill hides any lane line under the badge; the ring frames the chevron.
            ctx.fill(haloPath, with: .color(Color(nsColor: .windowBackgroundColor)))
            ctx.stroke(haloPath, with: .color(markerColor.opacity(0.35)), style: dagSolidStroke)
            for chevron in 0 ..< 2 {
                let x = tipX - CGFloat(chevron) * dagOverflowChevronGap
                let path = Path { p in
                    p.move(to: CGPoint(x: x - arm, y: nodeY - arm))
                    p.addLine(to: CGPoint(x: x, y: nodeY))
                    p.addLine(to: CGPoint(x: x - arm, y: nodeY + arm))
                }
                ctx.stroke(path, with: .color(markerColor), style: dagOverflowMarkerStroke)
            }
        }
    }

    private func overflowFadeMask(width: CGFloat) -> some View {
        let solid = max(0, (width - dagOverflowFadeWidth) / max(width, 1))
        return LinearGradient(
            stops: [
                .init(color: .black, location: 0),
                .init(color: .black, location: solid),
                .init(color: .clear, location: 1)
            ],
            startPoint: .leading,
            endPoint: .trailing
        )
    }

    private func strokeStyle(for cell: DagVerticalCell) -> StrokeStyle? {
        switch cell {
            case .empty: nil
            case .direct: dagSolidStroke
            case .indirect: dagIndirectEdgeStroke
        }
    }

    private func strokeStyle(for kind: DagEdgeKind) -> StrokeStyle {
        kind == .indirect ? dagIndirectEdgeStroke : dagSolidStroke
    }
}
