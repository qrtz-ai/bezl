// Linux typecheck only: the two Apple APIs main.swift uses on top of scripts/ctrl/mac-stubs.swift.
import Foundation
public typealias CFDictionary = NSDictionary
public let kAXTrustedCheckOptionPrompt: Unmanaged<CFString> = Unmanaged.passUnretained("AXTrustedCheckOptionPrompt" as NSString)
public func AXIsProcessTrustedWithOptions(_ options: CFDictionary?) -> Bool { false }
