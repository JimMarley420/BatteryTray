using System;
using System.Linq;
using BatteryTray.Core;
using BatteryTray.Providers;
using Xunit;
using static BatteryTray.Providers.SonixKeyboardProvider;

namespace BatteryTray.Tests
{
    /// <summary>
    /// The battery read of keyboards on the SONiX dongle 0C45:FEFE (Attack Shark X85, EWEADN X87 and others): 33-byte
    /// reports, report ID 0 first. The exchange is the one vigil-watt (docs/hardware/eweadn.md) measured on an EWEADN
    /// X87: 00 20 01 00 .. 21 out, 00 20 01 00 64 .. back.
    /// </summary>
    public sealed class SonixKeyboardTests
    {
        static byte[] Hex(string hex) => hex.Split(' ').Select(b => Convert.ToByte(b, 16)).ToArray();

        /// <summary>A 33-byte report starting with the given bytes, zero padded.</summary>
        static byte[] Report(string head)
        {
            var r = new byte[33];
            Hex(head).CopyTo(r, 0);
            return r;
        }

        [Fact]
        public void RequestIsTheCapturedOne()
        {
            byte[] expected = Report("00 20 01");
            expected[32] = 0x21;
            Assert.Equal(expected, Request());
        }

        [Fact]
        public void EweadnX87AtFull()
        {
            byte[] r = Report("00 20 01 00 64");
            r[32] = 0x85;
            Assert.Equal(100, ParseLevel(r));
        }

        [Fact]
        public void LevelInPercent()
        {
            Assert.Equal(73, ParseLevel(Report("00 20 01 00 49")));
            Assert.Equal(1, ParseLevel(Report("00 20 01 00 01")));
        }

        [Fact]
        public void ZeroIsAKeyboardThatIsOff()
        {
            Assert.Null(ParseLevel(Report("00 20 01 00 00")));
        }

        [Fact]
        public void TheRequestComingBackIsNoReading()
        {
            Assert.Null(ParseLevel(Request()));
        }

        [Fact]
        public void LevelOver100IsNoReading()
        {
            Assert.Null(ParseLevel(Report("00 20 01 00 65")));
            Assert.Null(ParseLevel(Report("00 20 01 00 FF")));
        }

        [Fact]
        public void OtherReportsAreNotTheReply()
        {
            Assert.False(IsReply(Report("00 55 10 14 00 00 01 00 00 40")));
            Assert.False(IsReply(Report("00 20 02 00 64")));
            Assert.False(IsReply(Report("00 20 01 01 64")));
            Assert.Null(ParseLevel(Report("00 0F 01 01 11")));
        }

        [Fact]
        public void AShortReportIsNotTheReply()
        {
            Assert.False(IsReply(Hex("00 20 01 00")));
            Assert.False(IsReply(Array.Empty<byte>()));
        }

        [Fact]
        public void KeyboardNameFromTheDongle()
        {
            Loc.Set("en");
            Assert.Equal("Keyboard on a 2.4G receiver", ReceiverName.Keyboard("2.4G Dongle", Loc.T("name.fa09Keyboard")));
            Assert.Equal("Neo Ergo", ReceiverName.Keyboard("Neo Ergo 2.4G"));
            Assert.Equal("AK820MAX keyboard", ReceiverName.Keyboard("AK820MAX 2.4G"));
            Assert.Equal("AJAZZ AK820MAX", ReceiverName.Keyboard("AJAZZ AK820MAX"));
            Assert.Equal("Keyboard", ReceiverName.Keyboard(null));
        }
    }
}
