import Foundation
import Testing
@testable import Codenotch

/// GUX-1 sizes each card's session list for that card. Its budget must be
/// the room the panel really takes on screen: at the small notch size the
/// notch-scaled budget promises more than a 982-point laptop screen holds,
/// and a busy side-edge panel ran 69 points off it (upstream's
/// OllamaPerformanceViewTests caught it in CI).
@MainActor @Suite struct PanelFitsTheScreenTests {
    @Test func aBusyPanelFitsTheScreenAtEverySizeAndEdge() throws {
        let names = ["deepseek-r1:1.5b", "gemma4:e4b", "llama3.2:1b", "ministral-3:3b", "qwen3:0.6b"]
        let data = try JSONSerialization.data(withJSONObject: ["models": names.map {
            ["name": $0, "size": 4_831_838_208, "context_length": 2048,
             "details": $0.hasPrefix("qwen") ? [:] : ["quantization_level": $0.hasPrefix("llama") ? "Q8_0" : "Q4_K_M"]] as [String: Any]
        }])
        let runtime = ProviderSnapshot(id: "ollama-local", displayName: "Ollama", glyph: .ollama, fidelity: .official,
                                       status: .ok, windows: [], kind: .localRuntime,
                                       localRuntime: try OllamaLocalUsage.parse(data))
        let model = NotchViewModel()
        model.setLocalMetricsEnabled(true)
        model.updateSnapshots([Fixtures.snapshots()[0], runtime])
        let measured = Date(timeIntervalSince1970: 1_700_000_000)
        model.now = measured.addingTimeInterval(120)
        model.updatePerformances(Dictionary(uniqueKeysWithValues: zip(names, [999, 30, 15, 5]).map { name, speed in
            (name, LocalModelPerformance(outputTokens: speed, durationNanoseconds: 1_000_000_000, measuredAt: measured)!)
        }))
        model.thinkingModels = [names[0]: measured]
        model.isExpanded = true
        model.screenSize = CGSize(width: 1512, height: 982)
        for size in NotchSize.allCases {
            model.sizeScale = size.scale
            for edge in NotchEdge.allCases {
                model.edge = edge
                model.hoveredIndex = 1
                #expect(model.panelSize.height <= model.screenSize.height + 0.1, "\(edge) \(size)")
            }
        }
    }
}
