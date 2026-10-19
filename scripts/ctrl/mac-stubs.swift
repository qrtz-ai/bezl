// Linux-only stand-ins for the Apple APIs QrtzCtrl/mac uses, written to the real Swift signatures (macOS 14 SDK) from
// memory; used by scripts/ctrl/mac-typecheck.sh. A clean typecheck here catches language-level mistakes (types,
// optionals, shadowing, exhaustiveness, protocol conformance), not SDK drift: only a Mac build proves the SDK side.
// Hand-written stubs of the Apple APIs used by QrtzCtrl/mac, with the real Swift signatures (macOS 14 SDK).
import Foundation
public typealias CFTypeRef = AnyObject
public typealias CFString = NSString
public typealias CFArray = NSArray
public typealias CFURL = NSURL
public final class AXUIElement {}
public typealias CFTypeID = UInt
public typealias CFIndex = Int
public typealias CFHashCode = UInt
public typealias pid_t = Int32
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
    kAXRoleDescriptionAttribute = "AXRoleDescription", kAXDocumentAttribute = "AXDocument"
// LaunchServices / AE
public struct LSRolesMask: OptionSet { public let rawValue: UInt32; public init(rawValue: UInt32) { self.rawValue = rawValue }; public static let all = LSRolesMask(rawValue: 0xFFFFFFFF) }
public final class CFError {}
public func LSCopyDefaultApplicationURLForURL(_ inURL: CFURL, _ inRoleMask: LSRolesMask, _ outError: UnsafeMutablePointer<Unmanaged<CFError>?>?) -> Unmanaged<CFURL>? { nil }
public typealias FourCharCode = UInt32
public typealias DescType = FourCharCode
public typealias AEEventClass = FourCharCode
public typealias AEEventID = FourCharCode
public typealias OSErr = Int16
public typealias OSStatus = Int32
public typealias Size = Int
public struct AEDesc { public var descriptorType: DescType; public init() { descriptorType = 0 } }
public typealias AEAddressDesc = AEDesc
public var typeApplicationBundleID: Int { 0 }
public var typeWildCard: Int { 0 }
public func AECreateDesc(_ typeCode: DescType, _ dataPtr: UnsafeRawPointer!, _ dataSize: Size, _ result: UnsafeMutablePointer<AEDesc>!) -> OSErr { 0 }
@discardableResult public func AEDisposeDesc(_ theAEDesc: UnsafeMutablePointer<AEDesc>!) -> OSErr { 0 }
public func AEDeterminePermissionToAutomateTarget(_ target: UnsafePointer<AEAddressDesc>!, _ theAEEventClass: AEEventClass, _ theAEEventID: AEEventID, _ askUserIfNeeded: Bool) -> OSStatus { 0 }
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
open class NSPasteboard: NSObject { open class var general: NSPasteboard { NSPasteboard() }; open var changeCount: Int { 0 }
    @discardableResult open func clearContents() -> Int { 0 } }
open class NSAppleEventDescriptor: NSObject { open var int32Value: Int32 { 0 }; open var stringValue: String? { nil } }
open class NSAppleScript: NSObject {
    public init?(source: String) {}
    open func executeAndReturnError(_ errorInfo: UnsafeMutablePointer<NSDictionary?>?) -> NSAppleEventDescriptor { NSAppleEventDescriptor() }
}
// CoreGraphics
public typealias CGKeyCode = UInt16
public typealias CGFloat = Double
public struct CGPoint { public var x: CGFloat; public var y: CGFloat }
public struct CGEventFlags: OptionSet { public let rawValue: UInt64; public init(rawValue: UInt64) { self.rawValue = rawValue }
    public static let maskCommand = CGEventFlags(rawValue: 1), maskShift = CGEventFlags(rawValue: 2), maskAlternate = CGEventFlags(rawValue: 4), maskControl = CGEventFlags(rawValue: 8) }
public enum CGEventSourceStateID: Int32 { case privateState = -1, combinedSessionState = 0, hidSystemState = 1 }
public enum CGEventType: UInt32 { case null = 0, keyDown = 10 }
public final class CGEventSource { public init?(stateID: CGEventSourceStateID) {}
    public class func secondsSinceLastEventType(_ stateID: CGEventSourceStateID, eventType: CGEventType) -> Double { 0 } }
public final class CGEvent { public init?(keyboardEventSource source: CGEventSource?, virtualKey: CGKeyCode, keyDown: Bool) {}
    public init?(source: CGEventSource?) {}
    public var flags: CGEventFlags = []
    public var location: CGPoint { CGPoint(x: 0, y: 0) }
    public func postToPid(_ pid: pid_t) {} }
public func CGPreflightPostEventAccess() -> Bool { false }
public typealias CGWindowID = UInt32
public let kCGNullWindowID: CGWindowID = 0
public struct CGWindowListOption: OptionSet { public let rawValue: UInt32; public init(rawValue: UInt32) { self.rawValue = rawValue }
    public static let optionOnScreenOnly = CGWindowListOption(rawValue: 1), excludeDesktopElements = CGWindowListOption(rawValue: 16) }
public func CGWindowListCopyWindowInfo(_ option: CGWindowListOption, _ relativeToWindow: CGWindowID) -> CFArray? { nil }
public let kCGWindowLayer: CFString = "", kCGWindowNumber: CFString = "", kCGWindowOwnerPID: CFString = ""
public func IsSecureEventInputEnabled() -> Bool { false }
public typealias CGDirectDisplayID = UInt32
public typealias boolean_t = Int32
public func CGMainDisplayID() -> CGDirectDisplayID { 0 }
public func CGDisplayIsAsleep(_ display: CGDirectDisplayID) -> boolean_t { 0 }
// IOKit (ADR 0180)
public typealias mach_port_t = UInt32
public typealias io_object_t = mach_port_t
public typealias io_registry_entry_t = io_object_t
public typealias kern_return_t = Int32
public let KERN_SUCCESS: kern_return_t = 0
public let kIOMainPortDefault: mach_port_t = 0
public let kCFBooleanTrue: CFTypeRef! = NSNumber(value: true)
public func IORegistryEntryFromPath(_ mainPort: mach_port_t, _ path: UnsafePointer<CChar>!) -> io_registry_entry_t { 0 }
public func IORegistryEntrySetCFProperty(_ entry: io_registry_entry_t, _ propertyName: CFString!, _ property: CFTypeRef!) -> kern_return_t { 0 }
@discardableResult public func IOObjectRelease(_ object: io_object_t) -> kern_return_t { 0 }
// IOKit power management + sysctl (ADR 0183)
public typealias io_connect_t = io_object_t
public typealias IOReturn = kern_return_t
public let kIOReturnSuccess: IOReturn = 0
public func IOPMFindPowerManagement(_ main_device_port: mach_port_t) -> io_connect_t { 0 }
public func IOPMSleepSystem(_ fb: io_connect_t) -> IOReturn { 0 }
@discardableResult public func IOServiceClose(_ connect: io_connect_t) -> kern_return_t { 0 }
public func sysctlbyname(_ name: UnsafePointer<CChar>!, _ oldp: UnsafeMutableRawPointer!, _ oldlenp: UnsafeMutablePointer<Int>!,
                         _ newp: UnsafeMutableRawPointer!, _ newlen: Int) -> Int32 { -1 }
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
