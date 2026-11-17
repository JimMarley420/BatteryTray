using System;
using System.Collections.Generic;
using System.Linq;
using BatteryTray.Core;
using BatteryTray.Hid;

namespace BatteryTray.Providers
{
    /// <summary>
    /// Wireless keyboards on the CompX 2.4 GHz receiver 3554:FA09 ("2.4G Wireless Receiver"), which MCHOSE (M HUB) and
    /// AULA keyboards, among others, share. The receiver's vendor collection FF02:0002 takes 20-byte output reports 13
    /// and answers with 20-byte input reports 13, the command echoed in byte 1:
    ///   13 4A 01 00 .. 00 5E   battery: reply 13 4A 01 00 xx LL SS ..   LL level in percent, SS high nibble 1 = charging
    ///   13 05 01 00 .. 00 19   model code: reply 13 05 01 00 xx C C C C C C ..   six bytes that name an MCHOSE model
    /// The last request byte is the sum of the others; MCHOSE firmware leaves the reply's last byte at 0, so it isn't
    /// checked. Some firmware sets the top bit of the echoed command. A keyboard that is asleep or off doesn't answer.
    /// Only these two reads are sent, the model code once per receiver. The keyboard is asked every 5 minutes, no more:
    /// a request may wake a dozing keyboard (Wireless-Device-Battery-Monitor issue 191), and its level changes slowly.
    /// Protocol facts from myfinder-device-battery (github.com/wushuai0802/myfinder-device-battery,
    /// engine/upstream-protocols.md, "MCHOSE BY keyboard receiver 3554:FA09": the official M HUB web driver read
    /// statically, and a G75 Pro owner's captured replies) and Wireless-Device-Battery-Monitor
    /// (github.com/krisgfx9/Wireless-Device-Battery-Monitor, src/drivers/aula.cpp; an MCHOSE K99 V2 and an AULA F99
    /// read by users in its issues 137 and 57); own implementation. Not yet verified on hardware by this project, which
    /// is why the first exchange goes to the log.
    /// </summary>
    public sealed class AulaMchoseProvider : IBatteryProvider
    {
        const ushort Vid = 0x3554, Pid = 0xFA09;
        const ushort VendorPage = 0xFF02, VendorUsage = 0x0002;
        const byte ReportId = 0x13, BatteryCommand = 0x4A, ModelCommand = 0x05;
        const int FrameLength = 20;
        const int TimeoutMs = 2500;
        const string Id = "compx:3554:fa09";

        internal sealed record Battery(int Level, bool Charging);

        /// <summary>M HUB's keyboardPwds table: the six bytes the model code request returns.</summary>
        static readonly (byte[] Code, string Name)[] Models =
        {
            (new byte[] { 0x03, 0x00, 0x00, 0x00, 0x02, 0x1C }, "MCHOSE G87"),
            (new byte[] { 0x03, 0x00, 0x00, 0x00, 0x00, 0x77 }, "MCHOSE K99"),
            (new byte[] { 0x03, 0x00, 0x00, 0x00, 0x01, 0xDF }, "MCHOSE K99 V2"),
            (new byte[] { 0x03, 0x00, 0x00, 0x00, 0x01, 0x32 }, "MCHOSE G98"),
            (new byte[] { 0x03, 0x00, 0x00, 0x00, 0x00, 0xD6 }, "MCHOSE X75"),
            (new byte[] { 0x0A, 0x00, 0x00, 0x00, 0x00, 0x03 }, "MCHOSE G98 Pro"),
            (new byte[] { 0x03, 0x00, 0x00, 0x00, 0x01, 0xC6 }, "MCHOSE X75 V2"),
            (new byte[] { 0x03, 0x00, 0x00, 0x00, 0x01, 0x15 }, "MCHOSE G75 Pro"),
            (new byte[] { 0x0A, 0x00, 0x00, 0x00, 0x00, 0x18 }, "MCHOSE G98 V2"),
            (new byte[] { 0x0A, 0x00, 0x00, 0x00, 0x00, 0x2B }, "MCHOSE UT98"),
            (new byte[] { 0x0A, 0x00, 0x00, 0x00, 0x00, 0x01 }, "MCHOSE Z75"),
            (new byte[] { 0x0A, 0x00, 0x00, 0x00, 0x00, 0x09 }, "MCHOSE Z75S"),
            (new byte[] { 0x03, 0x00, 0x00, 0x00, 0x01, 0xC7 }, "MCHOSE KX75"),
            (new byte[] { 0x03, 0x00, 0x00, 0x00, 0x01, 0x74 }, "MCHOSE GX87"),
            (new byte[] { 0x03, 0x00, 0x00, 0x00, 0x01, 0xED }, "MCHOSE GX87 V2"),
            (new byte[] { 0x03, 0x00, 0x00, 0x00, 0x01, 0x35 }, "MCHOSE K87"),
            (new byte[] { 0x03, 0x00, 0x00, 0x00, 0x03, 0x35 }, "MCHOSE G75 V2"),
            (new byte[] { 0x0A, 0x00, 0x00, 0x00, 0x00, 0x34 }, "MCHOSE G87 V2"),
        };

        /// <summary>The model each receiver answered with, by device path; null = it answered with a code not in the table.</summary>
        readonly Dictionary<string, string?> models = new();
        readonly HashSet<string> logged = new();

        public string Name => "AULA / MCHOSE";

        public IReadOnlyList<DeviceReading> Poll()
        {
            var result = new List<DeviceReading>();
            List<HidInfo> collections = HidBus.Enumerate((v, p) => v == Vid && p == Pid);
            if (collections.Count == 0) return result;

            HidInfo? vendor = collections.FirstOrDefault(i => i.UsagePage == VendorPage && i.Usage == VendorUsage
                && (i.AccessDenied || (i.OutputLength >= FrameLength && i.InputLength >= FrameLength)));
            var trace = new List<string>();
            Battery? battery = null;
            string? error = null;
            if (vendor == null)
                error = Loc.T("d.noLink");
            else if (vendor.AccessDenied)
                error = Loc.T("d.needsAdmin");
            else
            {
                try
                {
                    using var dev = new HidDevice(vendor);
                    if (!models.ContainsKey(vendor.Path) && Ask(dev, ModelCommand, trace) is byte[] code)
                        models[vendor.Path] = ModelName(code);
                    if (Ask(dev, BatteryCommand, trace) is byte[] reply) battery = ParseBattery(reply);
                }
                catch (Exception e)
                {
                    trace.Add("error: " + e.Message);
                    error = Loc.T("d.error", e.Message);
                }
            }
            string? model = vendor != null && models.TryGetValue(vendor.Path, out string? m) ? m : null;
            LogOnce(collections, vendor, trace, battery != null);

            result.Add(new DeviceReading
            {
                Id = Id, Name = model ?? Loc.T("name.fa09Keyboard"), Kind = DeviceKind.Keyboard, Via = Loc.T("via.24"),
                Online = battery != null, Level = battery?.Level, Charging = battery?.Charging ?? false,
                Detail = battery == null ? error ?? Loc.T("d.asleep") : null,
            });
            return result;
        }

        public TimeSpan Delay(IReadOnlyList<DeviceReading> last) => TimeSpan.FromMinutes(5);

        /// <summary>Sends one request and waits for its reply; null when the keyboard didn't answer.</summary>
        static byte[]? Ask(HidDevice dev, byte command, List<string> trace)
        {
            dev.Flush();
            byte[] request = Request(command);
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
                if (IsReply(r, command)) return r;
            }
            return null;
        }

        /// <summary>13 &lt;command&gt; 01 00 .. and the sum of the bytes before it, as M HUB sends it.</summary>
        internal static byte[] Request(byte command)
        {
            var r = new byte[FrameLength];
            r[0] = ReportId;
            r[1] = command;
            r[2] = 0x01;
            int sum = 0;
            for (int i = 0; i < FrameLength - 1; i++) sum += r[i];
            r[FrameLength - 1] = (byte)sum;
            return r;
        }

        /// <summary>The answer to <paramref name="command"/>: other reports 13 (status pushes) and a request that comes
        /// back as it went out, with nothing after the header, don't count.</summary>
        internal static bool IsReply(byte[] r, byte command)
        {
            if (r.Length < FrameLength || r[0] != ReportId || (r[1] & 0x7F) != command) return false;
            for (int i = 4; i < FrameLength - 1; i++)
                if (r[i] != 0) return true;
            return false;
        }

        /// <summary>What a battery reply says; null for a level of 0 (the receiver answering for a keyboard that's
        /// off) or over 100.</summary>
        internal static Battery? ParseBattery(byte[] r)
        {
            if (!IsReply(r, BatteryCommand)) return null;
            int level = r[5];
            if (level is < 1 or > 100) return null;
            return new Battery(level, r[6] >> 4 == 1);
        }

        /// <summary>The MCHOSE model a model code reply names, or null. M HUB reads the code at byte 5; one owner's
        /// capture had it a byte earlier, so byte 4 is tried too.</summary>
        internal static string? ModelName(byte[] r)
        {
            if (!IsReply(r, ModelCommand)) return null;
            foreach (int offset in new[] { 5, 4 })
                foreach (var (code, name) in Models)
                    if (r.AsSpan(offset, code.Length).SequenceEqual(code)) return name;
            return null;
        }

        /// <summary>
        /// Logs the receiver's HID collections and what was sent and received, the first time the keyboard is found off
        /// and the first time it is found on in a session, as the headsets and mice do.
        /// </summary>
        void LogOnce(List<HidInfo> collections, HidInfo? vendor, List<string> trace, bool online)
        {
            if (!logged.Add(online.ToString())) return;
            Log.Write($"AULA / MCHOSE {Vid:x4}:{Pid:x4}: found {(online ? "on" : "off")}, {collections.Count} HID collections");
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
