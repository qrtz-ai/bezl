// Linux-only stand-ins for the Apple APIs QrtzCtrl/mac uses, written to the real Swift signatures (macOS 14 SDK) from
// memory; used by scripts/ctrl/mac-typecheck.sh. A clean typecheck here catches language-level mistakes (types,
// optionals, shadowing, exhaustiveness, protocol conformance), not SDK drift: only a Mac build proves the SDK side.
// Hand-written stubs of the Apple APIs used by QrtzCtrl/mac, with the real Swift signatures (macOS 14 SDK).
import Foundation
public typealias CFTypeRef = AnyObject
public typealias CFString = NSString
public typealias CFArray = NSArray
public typealias CFURL = NSURL
public typealias CFDictionary = NSDictionary
public final class AXUIElement {}
public typealias CFTypeID = UInt
public typealias CFIndex = Int
public typealias CFHashCode = UInt
public typealias pid_t = Int32
public typealias CFBoolean = NSNumber
public typealias CFNumber = NSNumber
public final class AXValue {}
public func CFBooleanGetTypeID() -> CFTypeID { 0 }
public func CFNumberGetTypeID() -> CFTypeID { 0 }
public func CFURLGetTypeID() -> CFTypeID { 0 }
public func AXValueGetTypeID() -> CFTypeID { 0 }
public func CFBooleanGetValue(_ b: CFBoolean!) -> Bool { false }
public func CFEqual(_ a: CFTypeRef!, _ b: CFTypeRef!) -> Bool { false }
public func CFGetTypeID(_ cf: CFTypeRef!) -> CFTypeID { 0 }
public func CFStringGetTypeID() -> CFTypeID { 0 }
public func CFArrayGetTypeID() -> CFTypeID { 0 }
public func AXUIElementGetTypeID() -> CFTypeID { 0 }
public func CFHash(_ cf: CFTypeRef!) -> CFHashCode { 0 }
// bridging stand-ins: `x as CFString`, `cf as String`, `cf as NSArray`, `cf as URL` are written as functions in the
// rewritten source below.
public struct AXError: Equatable { public var rawValue: Int32; public static let success = AXError(rawValue: 0) }
public struct AXCopyMultipleAttributeOptions {}
public func AXUIElementCopyAttributeValue(_ element: AXUIElement, _ attribute: CFString, _ value: UnsafeMutablePointer<CFTypeRef?>) -> AXError { .success }
public func AXUIElementCopyActionNames(_ element: AXUIElement, _ names: UnsafeMutablePointer<CFArray?>) -> AXError { .success }
public func AXUIElementGetAttributeValueCount(_ element: AXUIElement, _ attribute: CFString, _ count: UnsafeMutablePointer<CFIndex>) -> AXError { .success }
public func AXUIElementPerformAction(_ element: AXUIElement, _ action: CFString) -> AXError { .success }
public func AXUIElementSetAttributeValue(_ element: AXUIElement, _ attribute: CFString, _ value: CFTypeRef) -> AXError { .success }
@discardableResult public func AXUIElementSetMessagingTimeout(_ element: AXUIElement, _ t: Float) -> AXError { .success }
public func AXUIElementCreateApplication(_ pid: pid_t) -> AXUIElement { AXUIElement() }
public func AXUIElementCreateSystemWide() -> AXUIElement { AXUIElement() }
@discardableResult public func AXUIElementGetPid(_ element: AXUIElement, _ pid: UnsafeMutablePointer<pid_t>) -> AXError { .success }
public func AXIsProcessTrusted() -> Bool { false }
public let kAXRoleAttribute = "AXRole", kAXSubroleAttribute = "AXSubrole", kAXTitleAttribute = "AXTitle",
    kAXDescriptionAttribute = "AXDescription", kAXChildrenAttribute = "AXChildren", kAXWindowsAttribute = "AXWindows",
    kAXFocusedWindowAttribute = "AXFocusedWindow", kAXMainWindowAttribute = "AXMainWindow",
    kAXCloseButtonAttribute = "AXCloseButton", kAXFocusedUIElementAttribute = "AXFocusedUIElement",
    kAXRoleDescriptionAttribute = "AXRoleDescription", kAXDocumentAttribute = "AXDocument",
    kAXMinimizedAttribute = "AXMinimized", kAXValueAttribute = "AXValue", kAXEnabledAttribute = "AXEnabled",
    kAXSizeAttribute = "AXSize", kAXFocusedAttribute = "AXFocused", kAXMenuBarAttribute = "AXMenuBar",
    kAXMenuItemCmdCharAttribute = "AXMenuItemCmdChar", kAXMenuItemCmdVirtualKeyAttribute = "AXMenuItemCmdVirtualKey",
    kAXMenuItemCmdModifiersAttribute = "AXMenuItemCmdModifiers", kAXVerticalScrollBarAttribute = "AXVerticalScrollBar"
public enum AXValueType: UInt32 { case cgPoint = 1, cgSize = 2 }
public func AXValueGetValue(_ value: AXValue, _ theType: AXValueType, _ valuePtr: UnsafeMutableRawPointer) -> Bool { false }
// LaunchServices / AE
public struct LSRolesMask: OptionSet { public let rawValue: UInt32; public init(rawValue: UInt32) { self.rawValue = rawValue }; public static let all = LSRolesMask(rawValue: 0xFFFFFFFF) }
public final class CFError {}
public func LSCopyDefaultApplicationURLForURL(_ inURL: CFURL, _ inRoleMask: LSRolesMask, _ outError: UnsafeMutablePointer<Unmanaged<CFError>?>?) -> Unmanaged<CFURL>? { nil }
// AppKit
public enum NSApplicationActivationPolicy { case regular, accessory, prohibited }
public struct NSApplicationActivationOptions: OptionSet { public let rawValue: UInt; public init(rawValue: UInt) { self.rawValue = rawValue } }
open class NSRunningApplication: NSObject {
    public convenience init?(processIdentifier pid: pid_t) { self.init() }
    open var bundleIdentifier: String? { nil }
    open var isTerminated: Bool { false }
    open var processIdentifier: pid_t { 0 }
    open var activationPolicy: NSApplicationActivationPolicy { .regular }
    open var localizedName: String? { nil }
    open var bundleURL: URL? { nil }
    open func activate(options: NSApplicationActivationOptions = []) -> Bool { false }
    open func terminate() -> Bool { false }
    open func hide() -> Bool { false }
    open func unhide() -> Bool { false }
    open var isHidden: Bool { false }
}
open class NSWorkspace: NSObject {
    public static let shared = NSWorkspace()
    open var runningApplications: [NSRunningApplication] { [] }
    open var frontmostApplication: NSRunningApplication? { nil }
    open var notificationCenter: NotificationCenter { NotificationCenter.default }
    public static let didActivateApplicationNotification = Notification.Name("NSWorkspaceDidActivateApplicationNotification")
    public static let applicationUserInfoKey = "NSWorkspaceApplicationKey"
    open func urlsForApplications(toOpen url: URL) -> [URL] { [] }
    open func urlForApplication(withBundleIdentifier bundleIdentifier: String) -> URL? { nil }
    open class OpenConfiguration: NSObject {
        open var activates = true, addsToRecentItems = true, promptsUserIfNeeded = true, hides = false
    }
    open func open(_ urls: [URL], withApplicationAt applicationURL: URL, configuration: NSWorkspace.OpenConfiguration, completionHandler: (@Sendable (NSRunningApplication?, (any Error)?) -> Void)? = nil) {}
    open func openApplication(at applicationURL: URL, configuration: NSWorkspace.OpenConfiguration, completionHandler: (@Sendable (NSRunningApplication?, (any Error)?) -> Void)? = nil) {}
    open func unmountAndEjectDevice(at url: URL) throws {}
}
// CoreGraphics
public typealias CGKeyCode = UInt16
public typealias CGFloat = Double
public struct CGSize { public var width: CGFloat; public var height: CGFloat; public static let zero = CGSize(width: 0, height: 0) }
public struct CGEventFlags: OptionSet { public let rawValue: UInt64; public init(rawValue: UInt64) { self.rawValue = rawValue }
    public static let maskCommand = CGEventFlags(rawValue: 1), maskShift = CGEventFlags(rawValue: 2), maskAlternate = CGEventFlags(rawValue: 4), maskControl = CGEventFlags(rawValue: 8) }
public enum CGEventSourceStateID: Int32 { case privateState = -1, combinedSessionState = 0, hidSystemState = 1 }
public enum CGEventType: UInt32 { case null = 0, leftMouseDown = 1, rightMouseDown = 3, keyDown = 10, scrollWheel = 22, otherMouseDown = 25,
    tapDisabledByTimeout = 0xFFFFFFFE, tapDisabledByUserInput = 0xFFFFFFFF }
public enum CGEventField: UInt32 { case keyboardEventKeycode = 9, eventSourceUserData = 42 }
public enum CGEventTapLocation: UInt32 { case cghidEventTap = 0, cgSessionEventTap, cgAnnotatedSessionEventTap }
public enum CGEventTapPlacement: UInt32 { case headInsertEventTap = 0, tailAppendEventTap }
public enum CGEventTapOptions: UInt32 { case defaultTap = 0, listenOnly = 1 }
public typealias CGEventMask = UInt64
public typealias CGEventTapProxy = OpaquePointer
// The SDK type is @convention(c); a stub class type can't be, so the check treats it as a plain function.
public typealias CGEventTapCallBack = (CGEventTapProxy, CGEventType, CGEvent, UnsafeMutableRawPointer?) -> Unmanaged<CGEvent>?
public final class CFMachPort {}
public final class CFRunLoopSource {}
public final class CFRunLoop {}
public final class CFAllocator {}
public let kCFAllocatorDefault: CFAllocator! = nil
public struct CFRunLoopMode { public static let commonModes = CFRunLoopMode() }
public func CFMachPortCreateRunLoopSource(_ allocator: CFAllocator!, _ port: CFMachPort!, _ order: CFIndex) -> CFRunLoopSource! { CFRunLoopSource() }
public func CFRunLoopGetMain() -> CFRunLoop! { CFRunLoop() }
public func CFRunLoopAddSource(_ rl: CFRunLoop!, _ source: CFRunLoopSource!, _ mode: CFRunLoopMode!) {}
public let kCFBooleanTrue: CFBoolean! = NSNumber(value: true)
public final class CGEventSource { public init?(stateID: CGEventSourceStateID) {}; public var userData: Int64 = 0 }
public final class CGEvent {
    public init?(keyboardEventSource source: CGEventSource?, virtualKey: CGKeyCode, keyDown: Bool) {}
    public var flags: CGEventFlags = []
    public func postToPid(_ pid: pid_t) {}
    public func getIntegerValueField(_ field: CGEventField) -> Int64 { 0 }
    public func setIntegerValueField(_ field: CGEventField, value: Int64) {}
    public class func tapCreate(tap: CGEventTapLocation, place: CGEventTapPlacement, options: CGEventTapOptions, eventsOfInterest: CGEventMask,
                                callback: CGEventTapCallBack, userInfo: UnsafeMutableRawPointer?) -> CFMachPort? { nil }
    public class func tapEnable(tap: CFMachPort, enable: Bool) {}
}
// Core ML
public enum MLComputeUnits { case cpuOnly, cpuAndGPU, all, cpuAndNeuralEngine }
open class MLModelConfiguration: NSObject { open var computeUnits: MLComputeUnits = .all }
@objc public enum MLMultiArrayDataType: Int { case double = 65600, float64 = 65601, float32 = 65568, float16 = 65552, int32 = 131104 }
open class MLMultiArray: NSObject { public init(shape: [NSNumber], dataType: MLMultiArrayDataType) throws {}; open var count: Int { 0 }; open subscript(idx: Int) -> NSNumber { get { NSNumber(value: 0) } set {} } }
open class MLFeatureValue: NSObject { public convenience init(string: String) { self.init() }; public convenience init(multiArray: MLMultiArray) { self.init() }; open var multiArrayValue: MLMultiArray? { nil } }
public protocol MLFeatureProvider { func featureValue(for featureName: String) -> MLFeatureValue? }
open class MLDictionaryFeatureProvider: NSObject, MLFeatureProvider { public init(dictionary: [String: Any]) throws {}; public func featureValue(for featureName: String) -> MLFeatureValue? { nil } }
open class MLFeatureDescription: NSObject {}
open class MLModelDescription: NSObject { open var inputDescriptionsByName: [String: MLFeatureDescription] { [:] }
    open var outputDescriptionsByName: [String: MLFeatureDescription] { [:] } }
open class MLModel: NSObject { public init(contentsOf url: URL, configuration: MLModelConfiguration) throws {}
    open var modelDescription: MLModelDescription { MLModelDescription() }
    open class func compileModel(at modelURL: URL) throws -> URL { modelURL }
    open class func compileModel(at modelURL: URL) async throws -> URL { modelURL }
    open func prediction(from input: MLFeatureProvider) throws -> MLFeatureProvider { input } }
public func CGPreflightListenEventAccess() -> Bool { false }
public func IsSecureEventInputEnabled() -> Bool { false }
