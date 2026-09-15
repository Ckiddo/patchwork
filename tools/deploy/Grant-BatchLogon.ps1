param([Parameter(Mandatory)][ValidatePattern('^S-1-[0-9-]+$')][string]$Sid)
$ErrorActionPreference='Stop'
# Add only this account's batch-logon right; preserve every existing policy entry.
Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Security.Principal;
public static class PatchworkBatchLogon {
    [StructLayout(LayoutKind.Sequential)] struct Attributes {
        public uint Length; public IntPtr RootDirectory, ObjectName;
        public uint Flags; public IntPtr SecurityDescriptor, SecurityQualityOfService;
    }
    [StructLayout(LayoutKind.Sequential)] struct UnicodeString {
        public ushort Length, MaximumLength; public IntPtr Buffer;
    }
    [DllImport("advapi32.dll")] static extern uint LsaOpenPolicy(IntPtr name, ref Attributes attrs, uint access, out IntPtr handle);
    [DllImport("advapi32.dll")] static extern uint LsaAddAccountRights(IntPtr handle, byte[] sid, UnicodeString[] rights, uint count);
    [DllImport("advapi32.dll")] static extern uint LsaClose(IntPtr handle);
    [DllImport("advapi32.dll")] static extern uint LsaNtStatusToWinError(uint status);
    static void Check(uint status) { if(status != 0) throw new Win32Exception((int)LsaNtStatusToWinError(status)); }
    public static void Grant(string sidText) {
        var sid = new SecurityIdentifier(sidText); var bytes = new byte[sid.BinaryLength]; sid.GetBinaryForm(bytes,0);
        var attrs = new Attributes { Length = (uint)Marshal.SizeOf(typeof(Attributes)) }; IntPtr handle;
        Check(LsaOpenPolicy(IntPtr.Zero,ref attrs,0x810,out handle));
        string name="SeBatchLogonRight"; IntPtr buffer=Marshal.StringToHGlobalUni(name);
        try { Check(LsaAddAccountRights(handle,bytes,new[]{new UnicodeString {Length=(ushort)(name.Length*2),MaximumLength=(ushort)((name.Length+1)*2),Buffer=buffer}},1)); }
        finally { Marshal.FreeHGlobal(buffer); LsaClose(handle); }
    }
}
'@
[PatchworkBatchLogon]::Grant($Sid)
