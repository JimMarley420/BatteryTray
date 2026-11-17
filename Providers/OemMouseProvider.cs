using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Linq;
using BatteryTray.Core;
using BatteryTray.Hid;

namespace BatteryTray.Providers
{
    /// <summary>
    /// Mice whose configuration tools speak one OEM command frame: VXE R1 mice (ATK HUB) and mice on Areson receivers
    /// (the OemDrv.exe that Redragon, Fantech and others ship). Both directions are 17-byte frames:
    ///   [0] report ID  [1] command  [2] status, 0 = ok  [3..4] address  [5] data length  [6..15] data  [16] checksum,
    /// the checksum making all 17 bytes add up to 0x55. The battery is command 04 (GetPower), and it is all this sends:
    ///   request  08 04 00 .. 00 49
    ///   reply    RR 04 00 00 00 LL PP CC ..   PP level in percent, CC charging (VXE also sends the voltage after it)
    /// A mouse that is asleep or off doesn't answer through its receiver. The frame travels two ways:
    ///   VXE: output report 08 on the vendor collection FF02:0002, the reply as input report 08 on the same collection.
    ///   Areson: feature report 08 on the FF02 collection, the reply as input report 09 on the FF01 collection of the
    ///   same interface (the configuration tool writes and reads on two handles).
    /// Protocol facts from OpenMouse (github.com/OpenMouse-Project/mouse-protocol: src/drivers/atk/hid.ts,
    /// src/endgame-gear/wireless.ts, docs/vxe-r1-pro-max-testing.md; the R1 PRO MAX was verified there),
    /// vxe-battery-tray (github.com/ivancacic/vxe-battery-tray, PROTOCOL.md; the R1 SE+ on Windows), redragon-mouse-linux
    /// (github.com/rajchauhan28/redragon-mouse-linux, PROTOCOL.md; the M693 on 25A7:FA7C) and Wireless-Device-Battery-Monitor
    /// (github.com/krisgfx9/Wireless-Device-Battery-Monitor: docs/areson.md, src/drivers/fantech.cpp; the Fantech Helios
    /// Go XD5 on the same receiver, which also answers in whole percent); own implementation. Not yet verified on
    /// hardware by this project, which is why the first exchange goes to the log.
    /// </summary>
    public sealed class OemMouseProvider : IBatteryProvider
    {
        const byte RequestId = 0x08, PowerCommand = 0x04;
        const int FrameLength = 17;
        const ushort CommandPage = 0xFF02, AresonReplyPage = 0xFF01;
        const int TimeoutMs = 800;

        internal enum Transport { Output, Feature }

        internal sealed record Battery(int Level, bool Charging);

        /// <param name="Name">A text key (see <see cref="Loc"/>) or a ready name.</param>
        sealed record Model(string Key, string Name, Transport Transport, bool Wireless);

        static readonly Dictionary<(ushort Vid, ushort Pid), Model> Models = new()
        {
            // 3554 is CompX's vendor ID, which Pulsar, VGN, Lamzu and others use too: only these products are claimed
            [(0x3554, 0xF58A)] = new("vxe:r1promax", "VXE R1 PRO MAX", Transport.Output, true),
            [(0x3554, 0xF58C)] = new("vxe:r1promax", "VXE R1 PRO MAX", Transport.Output, false),
            [(0x373B, 0x1085)] = new("vxe:r1se", "VXE R1 SE / SE+", Transport.Output, true),
            [(0x3554, 0xF58E)] = new("vxe:r1se", "VXE R1 SE / SE+", Transport.Output, true),
            [(0x3554, 0xF58F)] = new("vxe:r1se", "VXE R1 SE / SE+", Transport.Output, false),
            // Sold under many names (Redragon M693, Fantech Helios Go XD5, ...), so none is shown. The older FA07 / FA08
            // receivers send the level in tens and aren't claimed.
            [(0x25A7, 0xFA7C)] = new("areson:fa7c", "name.aresonMouse", Transport.Feature, true),
            [(0x25A7, 0xFA7B)] = new("areson:fa7c", "name.aresonMouse", Transport.Feature, false),
        };

        readonly HashSet<string> logged = new();

        public string Name => "VXE / Areson";

        public IReadOnlyList<DeviceReading> Poll()
        {
            var result = new List<DeviceReading>();
            var found = HidBus.Enumerate((v, p) => Models.ContainsKey((v, p)));
            foreach (var group in found.GroupBy(i => (i.VendorId, i.ProductId)))
            {
                Model model = Models[group.Key];
                List<HidInfo> collections = group.ToList();
                var (send, listen) = Channels(model.Transport, collections);
                var trace = new List<string>();
                Battery? battery = null;
                string? error = null;
                if (send == null || listen == null)
                    error = Loc.T("d.noLink");
                else if (send.AccessDenied || listen.AccessDenied)
                    error = Loc.T("d.needsAdmin");
                else
                {
                    try
                    {
                        battery = Query(model.Transport, send, listen, trace);
                    }
                    catch (Exception e)
                    {
                        trace.Add("error: " + e.Message);
                        error = Loc.T("d.error", e.Message);
                    }
                }
                LogOnce(group.Key, collections, send, listen, trace, battery != null);

                result.Add(new DeviceReading
                {
                    Id = model.Key, Name = Loc.T(model.Name), Kind = DeviceKind.Mouse,
                    Via = Loc.T(model.Wireless ? "via.24" : "via.cable"), Online = battery != null,
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

        /// <summary>The collection the request goes to and the one the reply comes back on.</summary>
        static (HidInfo? Send, HidInfo? Listen) Channels(Transport transport, List<HidInfo> collections)
        {
            if (transport == Transport.Output)
            {
                HidInfo? vendor = collections.FirstOrDefault(i => i.UsagePage == CommandPage
                    && (i.AccessDenied || (i.OutputLength >= FrameLength && i.InputLength >= FrameLength)));
                return (vendor, vendor);
            }
            HidInfo? send = collections.FirstOrDefault(i => i.UsagePage == CommandPage
                && (i.AccessDenied || i.FeatureLength >= FrameLength));
            HidInfo? listen = send == null ? null : collections.FirstOrDefault(i => i.UsagePage == AresonReplyPage
                && i.InterfaceNumber == send.InterfaceNumber && (i.AccessDenied || i.InputLength >= FrameLength));
            return (send, listen);
        }

        /// <summary>Asks for the battery; null when the mouse didn't answer or the reply says nothing about it.</summary>
        static Battery? Query(Transport transport, HidInfo send, HidInfo listen, List<string> trace)
        {
            byte[] request = Frame(RequestId, PowerCommand);
            if (transport == Transport.Output)
            {
                using var dev = new HidDevice(send);
                dev.Flush();
                trace.Add("> " + Hex(request));
                if (!dev.Write(request, TimeoutMs))
                {
                    trace.Add("(not sent)");
                    return null;
                }
                return Receive(dev, trace);
            }

            using var reader = new HidDevice(listen, readOnly: true);
            reader.Flush();
            using var writer = OpenForFeatures(send);
            var feature = new byte[Math.Max(send.FeatureLength, FrameLength)];
            request.CopyTo(feature, 0);
            trace.Add("feature > " + Hex(feature));
            if (!writer.SetFeature(feature))
            {
                trace.Add("(not taken)");
                return null;
            }
            return Receive(reader, trace);
        }

        static HidDevice OpenForFeatures(HidInfo info)
        {
            try
            {
                return new HidDevice(info, overlapped: false);
            }
            catch (Win32Exception)
            {
                return new HidDevice(info, featuresOnly: true);
            }
        }

        /// <summary>Waits for the reply to GetPower; other reports (DPI and profile button events) are skipped.</summary>
        static Battery? Receive(HidDevice dev, List<string> trace)
        {
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
                if (IsPowerReply(r)) return Parse(r);
            }
            return null;
        }

        /// <summary>A request frame: report ID, command, zeros and the checksum.</summary>
        internal static byte[] Frame(byte reportId, byte command)
        {
            var frame = new byte[FrameLength];
            frame[0] = reportId;
            frame[1] = command;
            frame[FrameLength - 1] = Checksum(frame);
            return frame;
        }

        static byte Checksum(byte[] frame)
        {
            int sum = 0;
            for (int i = 0; i < FrameLength - 1; i++) sum += frame[i];
            return (byte)(0x55 - sum);
        }

        /// <summary>A whole frame answering GetPower, with a checksum that adds up.</summary>
        internal static bool IsPowerReply(byte[] r) =>
            r.Length >= FrameLength && r[1] == PowerCommand && r[FrameLength - 1] == Checksum(r);

        /// <summary>
        /// What a GetPower reply says, report ID first; null when the receiver refused or has no level. 0% on battery
        /// is no reading either: a mouse that empty has switched itself off, so it's the receiver answering for it.
        /// </summary>
        internal static Battery? Parse(byte[] r)
        {
            if (!IsPowerReply(r) || r[2] != 0x00 || r[5] < 1) return null;
            int level = r[6];
            bool charging = r[5] >= 2 && r[7] != 0;
            if (level > 100 || (level == 0 && !charging)) return null;
            return new Battery(level, charging);
        }

        /// <summary>
        /// Logs the mouse's HID collections and what was sent and received, the first time it is found off and the first
        /// time it is found on in a session, as the headsets and SteelSeries mice do.
        /// </summary>
        void LogOnce((ushort Vid, ushort Pid) id, List<HidInfo> collections, HidInfo? send, HidInfo? listen,
            List<string> trace, bool online)
        {
            string key = $"{id.Vid:x4}:{id.Pid:x4}";
            if (!logged.Add(key + online)) return;
            Log.Write($"VXE / Areson {key}: found {(online ? "on" : "off")}, {collections.Count} HID collections");
            foreach (HidInfo info in collections)
            {
                string role = info == send && info == listen ? "request and reply"
                    : info == send ? "request" : info == listen ? "reply" : "not used";
                Log.Write($"  mi={info.InterfaceNumber} {info}: {role}");
            }
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
