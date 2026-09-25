//
//  MarkdownRenderer.swift
//  ClaudeControl
//
//  Claude's markdown as SwiftUI text, parsed with swift-markdown: headings,
//  paragraphs with inline emphasis, code, links and strikethrough, lists,
//  quotes, code blocks and rules. Type and colours come from the theme.
//

import Markdown
import SwiftUI

// MARK: - Document cache

/// Parsed documents by text, so a redraw doesn't parse again.
private final class DocumentCache: @unchecked Sendable {
    static let shared = DocumentCache()
    private var cache: [String: Document] = [:]
    private let lock = NSLock()
    private let maxSize = 100

    func document(for text: String) -> Document {
        lock.lock()
        defer { lock.unlock() }
        if let cached = cache[text] { return cached }
        let document = Document(parsing: text, options: [.parseBlockDirectives, .parseSymbolLinks])
        if cache.count >= maxSize { cache.removeAll() }
        cache[text] = document
        return document
    }
}

// MARK: - Markdown text

struct MarkdownText: View {
    let text: String
    let token: ClaudeFontToken
    let ink: ClaudeInk.Token

    private let document: Document

    init(_ text: String, token: ClaudeFontToken = .chat, ink: ClaudeInk.Token = .primary) {
        self.text = text
        self.token = token
        self.ink = ink
        self.document = DocumentCache.shared.document(for: text)
    }

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        let children = Array(document.children)
        let style = MarkdownStyle(font: theme.font(token), ink: ink)
        if children.isEmpty {
            SwiftUI.Text(text)
                .font(style.font)
                .foregroundStyle(.ink(ink))
        } else {
            VStack(alignment: .leading, spacing: 8) {
                ForEach(Array(children.enumerated()), id: \.offset) { _, child in
                    BlockRenderer(markup: child, style: style)
                }
            }
        }
    }
}

/// The font and ink a block renders with.
private struct MarkdownStyle {
    let font: Font
    let ink: ClaudeInk.Token

    var code: Font { font.monospaced() }
}

// MARK: - Blocks

private struct BlockRenderer: View {
    let markup: Markup
    let style: MarkdownStyle

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        if let paragraph = markup as? Paragraph {
            InlineText(children: Array(paragraph.inlineChildren), style: style).text
                .lineSpacing(2.5)
                .fixedSize(horizontal: false, vertical: true)
        } else if let heading = markup as? Heading {
            InlineText(children: Array(heading.inlineChildren), style: style).text
                .fontWeight(heading.level <= 2 ? .bold : .semibold)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.top, heading.level <= 2 ? 3 : 1)
                .accessibilityAddTraits(.isHeader)
        } else if let codeBlock = markup as? CodeBlock {
            CodeBlockView(code: codeBlock.code, font: style.code)
        } else if let quote = markup as? BlockQuote {
            HStack(alignment: .top, spacing: 8) {
                Rectangle()
                    .fill(.ink(.separator))
                    .frame(width: 2)
                VStack(alignment: .leading, spacing: 4) {
                    ForEach(Array(quote.children.enumerated()), id: \.offset) { _, child in
                        BlockRenderer(markup: child, style: MarkdownStyle(font: style.font, ink: .secondary))
                    }
                }
            }
            .fixedSize(horizontal: false, vertical: true)
        } else if let list = markup as? UnorderedList {
            listView(Array(list.listItems), marker: { _ in "•" })
        } else if let list = markup as? OrderedList {
            let start = Int(list.startIndex)
            listView(Array(list.listItems), marker: { "\(start + $0)." })
        } else if markup is ThematicBreak {
            Rectangle()
                .fill(.ink(.separator))
                .frame(height: theme.hairline)
                .padding(.vertical, 3)
        } else {
            EmptyView()
        }
    }

    private func listView(_ items: [ListItem], marker: @escaping (Int) -> String) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            ForEach(Array(items.enumerated()), id: \.offset) { index, item in
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    SwiftUI.Text(marker(index))
                        .font(style.font)
                        .foregroundStyle(.ink(.secondary))
                        .monospacedDigit()
                    VStack(alignment: .leading, spacing: 3) {
                        ForEach(Array(item.children.enumerated()), id: \.offset) { _, child in
                            BlockRenderer(markup: child, style: style)
                        }
                    }
                }
            }
        }
    }
}

// MARK: - Inline

private struct InlineText {
    let children: [InlineMarkup]
    let style: MarkdownStyle

    var text: SwiftUI.Text {
        children.reduce(SwiftUI.Text("")) { $0 + render($1) }
            .font(style.font)
    }

    private func render(_ inline: InlineMarkup) -> SwiftUI.Text {
        switch inline {
        case let text as Markdown.Text:
            return SwiftUI.Text(text.string).foregroundStyle(.ink(style.ink))
        case let strong as Strong:
            return strong.inlineChildren.reduce(SwiftUI.Text("")) { $0 + render($1) }.bold()
        case let emphasis as Emphasis:
            return emphasis.inlineChildren.reduce(SwiftUI.Text("")) { $0 + render($1) }.italic()
        case let strike as Strikethrough:
            return strike.inlineChildren.reduce(SwiftUI.Text("")) { $0 + render($1) }.strikethrough()
        case let code as InlineCode:
            return SwiftUI.Text(code.code)
                .font(style.code)
                .foregroundStyle(.ink(style.ink))
        case let link as Markdown.Link:
            return SwiftUI.Text(link.plainText)
                .foregroundStyle(.ink(.accent))
                .underline()
        case is SoftBreak:
            return SwiftUI.Text(" ")
        case is LineBreak:
            return SwiftUI.Text("\n")
        default:
            return SwiftUI.Text(inline.plainText).foregroundStyle(.ink(style.ink))
        }
    }
}

// MARK: - Code block

private struct CodeBlockView: View {
    let code: String
    let font: Font

    @Environment(\.claudeControlTheme) private var theme

    var body: some View {
        SwiftUI.Text(code.hasSuffix("\n") ? String(code.dropLast()) : code)
            .font(font)
            .foregroundStyle(.ink(.primary))
            .fixedSize(horizontal: false, vertical: true)
            .textSelection(.enabled)
            .padding(.horizontal, 9)
            .padding(.vertical, 7)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: 8, style: .continuous).fill(theme.controlFill))
    }
}
