using System;
using System.Collections.Generic;
using System.Linq;
using BatteryTray.Core;
using BatteryTray.Hid;

namespace BatteryTray.Providers
{
    /// <summary>
    /// Mice with the product ID 2255 under the vendor IDs A8A5 (2.4 GHz receiver) and A8A4 (USB cable): the AJAZZ
    /// AJ139 V2 MC, the K-Snake X11, the Noir M2-NEX, the MCHOSE G7 Pro and others. They share one configuration
    /// protocol on the vendor collection FF01:0010, 64-byte reports with no report ID in both directions:
    ///   request  55 30 A5 CC 2E 00 ..      30 = battery, CC = the low byte of the sum of the bytes after it
    ///   reply    AA 30 .. [8] level in percent, [9] not 0 on USB power
    /// The mouse also sends reports of its own (AA FA, AA ED), which are skipped. A receiver whose mouse is off or
    /// out of range answers with a level of 0. The vendor's web panel writes the request differently,
    /// 55 30 A5 0B 2E 01 01, so that form is tried when the first one gets no answer, and the one that worked goes
    /// first the next time. Only this read is sent.
    /// The IDs don't tell the models apart, so the name comes from the product string (see <see cref="ReceiverName"/>).
    /// Protocol facts from ajazz-aj139-macos (github.com/werquess/ajazz-aj139-macos, PROTOCOL.md: checked on an AJ139
    /// V2 MC), OpenMouse (github.com/OpenMouse-Project/mouse-protocol: src/ksnake/index.ts, the vendor panel's form of
    /// the request, read on a Noir M2-NEX) and Wireless-Device-Battery-Monitor
    /// (github.com/krisgfx9/Wireless-Device-Battery-Monitor, src/drivers/misc_mice.cpp: both forms in this order; an
    /// AJ139 V2 MC and an MCHOSE G7 Pro read by users in its issues 40 and 226). Own implementation. Not yet verified
    /// on hardware by this project, which is why the first exchange goes to the log.
    /// </summary>
    public sealed class AjazzMouseProvider : IBatteryProvider
    {
        const ushort ReceiverVid = 0xA8A5, CableVid = 0xA8A4, Pid = 0x2255;
        const ushort VendorPage = 0xFF01, VendorUsage = 0x0010;
        const byte RequestHead = 0x55, ReplyHead = 0xAA, ReadMark = 0xA5, BatteryCommand = 0x30, BatteryLength = 0x2E;
        const int ReportLength = 65; // the report ID, always 0, and 64 bytes
        const int RequestForms = 2;
        const int TimeoutMs = 700;
        const string Id = "ajazz:2255";

        internal sealed record Battery(int Level, bool Charging);

        /// <summary>The request form each mouse answered last, by device path: it is the one tried first.</summary>
        readonly Dictionary<string, int> answered = new();
        readonly HashSet<string> logged = new();

        public string Name => "AJAZZ / K-Snake";

        public IReadOnlyList<DeviceReading> Poll()
        {
            var result = new List<DeviceReading>();
            var found = HidBus.Enumerate((v, p) => p == Pid && (v == ReceiverVid || v == CableVid));
            foreach (var group in found.GroupBy(i => i.VendorId))
            {
                bool cable = group.Key == CableVid;
                List<HidInfo> collections = group.ToList();
                HidInfo? vendor = collections.FirstOrDefault(i => i.UsagePage == VendorPage && i.Usage == VendorUsage
                    && i.OutputLength >= ReportLength && i.InputLength >= ReportLength);
                var trace = new List<string>();
                Battery? battery = null;
                string? error = null;
                if (vendor == null)
                    error = Loc.T(HidCollections.NoLinkReason(collections));
                else
                {
                    try
                    {
                        battery = Query(vendor, trace);
                    }
                    catch (Exception e)
                    {
                        trace.Add("error: " + e.Message);
                        error = Loc.T("d.error", e.Message);
                    }
                }
                string? product = vendor?.Product ?? collections.FirstOrDefault(i => i.Product != null)?.Product;
                LogOnce(group.Key, collections, vendor, product, trace, battery != null);

                result.Add(new DeviceReading
                {
                    Id = Id, Name = ReceiverName.Mouse(product), Kind = DeviceKind.Mouse,
                    Via = Loc.T(cable ? "via.cable" : "via.24"), Online = battery != null,
                    Level = battery?.Level, Charging = battery?.Charging ?? false,
                    Detail = battery == null ? error ?? Loc.T("d.asleep") : null,
                });
            }
            // A mouse charging on its cable while its receiver stays plugged in is listed twice under one Id, and the
            // receiver finds it off: keep the reading that reached the mouse
            return result.GroupBy(r => r.Id).Select(g => g.FirstOrDefault(r => r.Online) ?? g.First()).ToList();
        }

        public TimeSpan Delay(IReadOnlyList<DeviceReading> last)
        {
            return last.Any(r => r.Online) ? TimeSpan.FromSeconds(60) : TimeSpan.FromSeconds(30);
        }

        /// <summary>Asks for the battery; null when nothing answered or the reply has no level.</summary>
        Battery? Query(HidInfo vendor, List<string> trace)
        {
            using var dev = new HidDevice(vendor);
            int first = answered.TryGetValue(vendor.Path, out int form) ? form : 0;
            for (int i = 0; i < RequestForms; i++)
            {
                int tried = (first + i) % RequestForms;
                if (Ask(dev, Request(tried), trace) is byte[] reply)
                {
                    answered[vendor.Path] = tried;
                    return Parse(reply);
                }
            }
            return null;
        }

        /// <summary>Sends the request and waits for the battery reply; null when it didn't come.</summary>
        static byte[]? Ask(HidDevice dev, byte[] request, List<string> trace)
        {
            dev.Flush();
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
                if (IsBatteryReply(r)) return r;
            }
            return null;
        }

        /// <summary>
        /// The battery request, report ID first. Form 0 has the checksum the protocol notes describe, form 1 is what
        /// the vendor's web panel sends.
        /// </summary>
        internal static byte[] Request(int form)
        {
            var r = new byte[ReportLength];
            r[1] = RequestHead;
            r[2] = BatteryCommand;
            r[3] = ReadMark;
            r[5] = BatteryLength;
            if (form == 0)
            {
                int sum = 0;
                for (int i = 5; i < ReportLength; i++) sum += r[i];
                r[4] = (byte)sum;
            }
            else
            {
                r[4] = 0x0B;
                r[6] = 0x01;
                r[7] = 0x01;
            }
            return r;
        }

        /// <summary>The answer to the battery request, report ID first, and not one of the mouse's own reports.</summary>
        internal static bool IsBatteryReply(byte[] r) => r.Length >= 11 && r[1] == ReplyHead && r[2] == BatteryCommand;

        /// <summary>
        /// What a battery reply says; null for a level over 100 and for 0 off the cable, which is the receiver
        /// answering for a mouse it can't reach.
        /// </summary>
        internal static Battery? Parse(byte[] r)
        {
            if (!IsBatteryReply(r)) return null;
            int level = r[9];
            bool charging = r[10] != 0;
            if (level > 100 || (level == 0 && !charging)) return null;
            return new Battery(level, charging);
        }

        /// <summary>
        /// Logs the mouse's HID collections and what was sent and received, the first time it is found off and the
        /// first time it is found on in a session, as the other mice do.
        /// </summary>
        void LogOnce(ushort vid, List<HidInfo> collections, HidInfo? vendor, string? product, List<string> trace,
            bool online)
        {
            string key = $"{vid:x4}:{Pid:x4}";
            if (!logged.Add(key + online)) return;
            Log.Write($"AJAZZ / K-Snake {key}: found {(online ? "on" : "off")}, {collections.Count} HID collections, "
                + $"product \"{product}\"");
            foreach (HidInfo info in collections)
                Log.Write($"  mi={info.InterfaceNumber} {info}: {(info == vendor ? "asked" : "not used")}");
            Log.Write("  " + (trace.Count == 0 ? "nothing sent" : string.Join("  ", trace)));
        }

        /// <summary>A report for the log: without the zeros that pad it, and no more than its first 32 bytes.</summary>
        static string Hex(byte[] report)
        {
            int used = Array.FindLastIndex(report, b => b != 0) + 1;
            if (used == 0) return "00";
            string hex = BitConverter.ToString(report, 0, Math.Min(used, 32)).Replace('-', ' ').ToLowerInvariant();
            return used > 32 ? hex + " ..." : hex;
        }
    }
}
