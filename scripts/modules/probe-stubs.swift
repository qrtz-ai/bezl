// Linux-only stand-ins for the Apple APIs gate-probe.swift uses, written to the macOS 14 SDK's Swift signatures from
// memory; used by probe-typecheck.sh (the scripts/ctrl/mac-stubs.swift pattern). A clean typecheck catches language
// mistakes, not SDK drift: mac-gate.sh compiling the probe on the Mac is the real check.
import Foundation
public typealias CFString = NSString
public typealias CFDictionary = NSDictionary
// ApplicationServices
public func AXIsProcessTrusted() -> Bool { false }
public func AXIsProcessTrustedWithOptions(_ options: CFDictionary?) -> Bool { false }
public let kAXTrustedCheckOptionPrompt: Unmanaged<CFString> = Unmanaged.passRetained(NSString(string: "AXTrustedCheckOptionPrompt"))
// AVFoundation
public struct AVMediaType: Hashable { public let rawValue: String; public static let audio = AVMediaType(rawValue: "soun") }
public enum AVAuthorizationStatus: Int { case notDetermined = 0, restricted, denied, authorized }
open class AVCaptureDevice: NSObject {
    open class func authorizationStatus(for mediaType: AVMediaType) -> AVAuthorizationStatus { .notDetermined }
    open class func requestAccess(for mediaType: AVMediaType, completionHandler handler: @escaping @Sendable (Bool) -> Void) {}
    open class func `default`(for mediaType: AVMediaType?) -> AVCaptureDevice? { nil }
}
open class AVCaptureDeviceInput: NSObject { public init(device: AVCaptureDevice) throws {} }
// CoreServices (Apple Events)
public typealias FourCharCode = UInt32
public typealias DescType = FourCharCode
public typealias AEEventClass = FourCharCode
public typealias AEEventID = FourCharCode
public typealias OSErr = Int16
public typealias OSStatus = Int32
public typealias Size = Int
public var noErr: Int { 0 }
public struct AEDesc { public var descriptorType: DescType; public init() { descriptorType = 0 } }
public typealias AEAddressDesc = AEDesc
public var typeApplicationBundleID: Int { 0 }
public var typeWildCard: Int { 0 }
public func AECreateDesc(_ typeCode: DescType, _ dataPtr: UnsafeRawPointer!, _ dataSize: Size, _ result: UnsafeMutablePointer<AEDesc>!) -> OSErr { 0 }
@discardableResult public func AEDisposeDesc(_ theAEDesc: UnsafeMutablePointer<AEDesc>!) -> OSErr { 0 }
public func AEDeterminePermissionToAutomateTarget(_ target: UnsafePointer<AEAddressDesc>!, _ theAEEventClass: AEEventClass, _ theAEEventID: AEEventID, _ askUserIfNeeded: Bool) -> OSStatus { 0 }
// The SDK takes an AutoreleasingUnsafeMutablePointer (ObjC only); `&err` passes to either.
open class NSAppleScript: NSObject {
    public static let errorNumber = "NSAppleScriptErrorNumber"
    public init?(source: String) {}
    open func executeAndReturnError(_ errorInfo: UnsafeMutablePointer<NSDictionary?>?) -> NSAppleEventDescriptor { NSAppleEventDescriptor() }
}
open class NSAppleEventDescriptor: NSObject {}
// XPC
public final class _XPCObject {}
public typealias xpc_object_t = _XPCObject
public typealias xpc_connection_t = _XPCObject
public typealias xpc_type_t = OpaquePointer
public typealias xpc_handler_t = (xpc_object_t) -> Void
public let XPC_TYPE_ERROR: xpc_type_t = OpaquePointer(bitPattern: 1)!
public let XPC_ERROR_CONNECTION_INVALID: xpc_object_t = _XPCObject()
public func xpc_connection_create_mach_service(_ name: UnsafePointer<CChar>, _ targetq: DispatchQueue?, _ flags: UInt64) -> xpc_connection_t { _XPCObject() }
public func xpc_connection_set_event_handler(_ connection: xpc_connection_t, _ handler: @escaping xpc_handler_t) {}
public func xpc_connection_resume(_ connection: xpc_connection_t) {}
public func xpc_connection_cancel(_ connection: xpc_connection_t) {}
public func xpc_dictionary_create(_ keys: UnsafePointer<UnsafePointer<CChar>>?, _ values: UnsafePointer<xpc_object_t?>?, _ count: Int) -> xpc_object_t { _XPCObject() }
public func xpc_dictionary_set_string(_ xdict: xpc_object_t, _ key: UnsafePointer<CChar>, _ string: UnsafePointer<CChar>) {}
public func xpc_connection_send_message_with_reply_sync(_ connection: xpc_connection_t, _ message: xpc_object_t) -> xpc_object_t { _XPCObject() }
public func xpc_get_type(_ object: xpc_object_t) -> xpc_type_t { XPC_TYPE_ERROR }
public func xpc_equal(_ object1: xpc_object_t, _ object2: xpc_object_t) -> Bool { false }
