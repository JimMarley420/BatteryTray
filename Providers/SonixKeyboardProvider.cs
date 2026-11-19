using System;
using System.Collections.Generic;
using System.Linq;
using BatteryTray.Core;
using BatteryTray.Hid;

namespace BatteryTray.Providers
{
    /// <summary>
    /// Wireless keyboards on the SONiX 2.4 GHz dongle 0C45:FEFE ("2.4G Dongle"): the Attack Shark X85 (issue #21),
    /// the EWEADN X87, AJAZZ AK-series boards and others. The dongle has two vendor collections with the usage
    /// FF60:0061; the one with 33-byte reports (report ID 0 and 32 bytes) takes the battery request, and it is all
    /// this sends:
    ///   request  20 01 00 .. 00 21      the last byte is the sum of the others
    ///   reply    20 01 00 LL ..         LL level in percent
    /// The vendor's Windows tool for these boards asks the same way every 15 s and reads nothing but the level, so
    /// there is no charging sign. A keyboard that is asleep or off doesn't answer, or the dongle answers for it with
    /// a level of 0. Some boards that share these IDs have other firmware and never give a level this way.
    /// Protocol facts from vigil-watt (github.com/evren-oct/vigil-watt, docs/hardware/eweadn.md: the EWEADN X87's
    /// vendor tool read statically and the exchange measured on its 0C45:FEFE dongle; the X87 has the X85's USB IDs
    /// and the same vendor tool), sonix-keyboard-studio (github.com/donglover69/sonix-keyboard-studio) and
    /// Wireless-Device-Battery-Monitor (github.com/krisgfx9/Wireless-Device-Battery-Monitor, src/drivers/sonix.cpp).
    /// Own implementation. Not yet verified on hardware by this project, which is why the first exchange goes to the
    /// log.
    /// </summary>
    public sealed class SonixKeyboardProvider : IBatteryProvider
    {
        const ushort Vid = 0x0C45, Pid = 0xFEFE;
        const ushort VendorPage = 0xFF60, VendorUsage = 0x0061;
        const byte BatteryCommand = 0x20, BatteryArgument = 0x01;
        const int ReportLength = 33; // the report ID, always 0, and 32 bytes
        const int TimeoutMs = 500;
        const string Id = "sonix:0c45:fefe";

        readonly HashSet<string> logged = new();

        public string Name => "SONiX 2.4G";

        public IReadOnlyList<DeviceReading> Poll()
        {
            var result = new List<DeviceReading>();
            List<HidInfo> collections = HidBus.Enumerate((v, p) => v == Vid && p == Pid);
            if (collections.Count == 0) return result;

            // The other FF60:0061 collection has 65-byte reports and takes no battery request
            HidInfo? vendor = collections.FirstOrDefault(i => i.UsagePage == VendorPage && i.Usage == VendorUsage
                && i.OutputLength == ReportLength && i.InputLength == ReportLength);
            var trace = new List<string>();
            int? level = null;
            string? error = null;
            if (vendor == null)
                // A collection that Windows won't open shows no usages, so it can't be told from the others
                error = Loc.T(collections.Any(i => i.AccessDenied) ? "d.needsAdmin" : "d.noLink");
            else
            {
                try
                {
                    using var dev = new HidDevice(vendor);
                    // The first request can go unanswered while the link wakes up
                    byte[]? reply = Ask(dev, trace) ?? Ask(dev, trace);
                    if (reply != null) level = ParseLevel(reply);
                }
                catch (Exception e)
                {
                    trace.Add("error: " + e.Message);
                    error = Loc.T("d.error", e.Message);
                }
            }
            string? product = vendor?.Product ?? collections.FirstOrDefault(i => i.Product != null)?.Product;
            LogOnce(collections, vendor, product, trace, level != null);

            result.Add(new DeviceReading
            {
                Id = Id, Name = ReceiverName.Keyboard(product, Loc.T("name.fa09Keyboard")), Kind = DeviceKind.Keyboard,
                Via = Loc.T("via.24"), Online = level != null, Level = level,
                Detail = level == null ? error ?? Loc.T("d.asleep") : null,
            });
            return result;
        }

        /// <summary>The level changes slowly; a keyboard that didn't answer is asked again sooner, to catch it waking up.</summary>
        public TimeSpan Delay(IReadOnlyList<DeviceReading> last)
        {
            return last.Any(r => r.Online) ? TimeSpan.FromMinutes(5) : TimeSpan.FromMinutes(1);
        }

        /// <summary>Sends the battery request and waits for its reply; null when it didn't come.</summary>
        static byte[]? Ask(HidDevice dev, List<string> trace)
        {
            dev.Flush();
            byte[] request = Request();
            trace.Add("> " + Hex(request));
            if (!dev.Write(request, TimeoutMs))
            {
                trace.Add("(not sent)");
                return null;
            }
            DateTime deadline = DateTime.UtcNow.AddMilliseconds(TimeoutMs);
            while (DateTime.UtcNow < deadline)
            {
                byte[]? r = dev.Read(Math.Max((int)(deadline - DateTime.UtcNow).TotalMilliseconds, 1));
                if (r == null)
                {
                    trace.Add("< nothing");
                    return null;
                }
                trace.Add("< " + Hex(r));
                if (IsReply(r)) return r;
            }
            return null;
        }

        /// <summary>00 20 01 00 .. and the sum of the bytes before it, as the vendor tool sends it.</summary>
        internal static byte[] Request()
        {
            var r = new byte[ReportLength];
            r[1] = BatteryCommand;
            r[2] = BatteryArgument;
            int sum = 0;
            for (int i = 0; i < ReportLength - 1; i++) sum += r[i];
            r[ReportLength - 1] = (byte)sum;
            return r;
        }

        /// <summary>The answer to the battery request, report ID first: the command comes back with a 0 after it.</summary>
        internal static bool IsReply(byte[] r) =>
            r.Length >= 5 && r[1] == BatteryCommand && r[2] == BatteryArgument && r[3] == 0x00;

        /// <summary>The level a reply carries; null for 0 (the dongle answering for a keyboard that's off, or the
        /// request coming back) and for anything over 100.</summary>
        internal static int? ParseLevel(byte[] r)
        {
            if (!IsReply(r)) return null;
            int level = r[4];
            return level is >= 1 and <= 100 ? level : null;
        }

        /// <summary>
        /// Logs the dongle's HID collections and what was sent and received, the first time the keyboard is found off
        /// and the first time it is found on in a session, as the other keyboards and mice do.
        /// </summary>
        void LogOnce(List<HidInfo> collections, HidInfo? vendor, string? product, List<string> trace, bool online)
        {
            if (!logged.Add(online.ToString())) return;
            Log.Write($"SONiX {Vid:x4}:{Pid:x4}: found {(online ? "on" : "off")}, {collections.Count} HID collections, "
                + $"product \"{product}\"");
            foreach (HidInfo info in collections)
                Log.Write($"  mi={info.InterfaceNumber} {info}: {(info == vendor ? "asked" : "not used")}");
            Log.Write("  " + (trace.Count == 0 ? "nothing sent" : string.Join("  ", trace)));
        }

        /// <summary>A report for the log: without the zeros that pad it, and no more than its first 33 bytes.</summary>
        static string Hex(byte[] report)
        {
            int used = Array.FindLastIndex(report, b => b != 0) + 1;
            if (used == 0) return "00";
            string hex = BitConverter.ToString(report, 0, Math.Min(used, 33)).Replace('-', ' ').ToLowerInvariant();
            return used > 33 ? hex + " ..." : hex;
        }
    }
}
