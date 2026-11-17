using System;
using System.Linq;
using BatteryTray.Providers;
using Xunit;
using static BatteryTray.Providers.OemMouseProvider;

namespace BatteryTray.Tests
{
    /// <summary>
    /// GetPower in the OEM frame of VXE (ATK) and Areson mice: 17 bytes, report ID first, adding up to 0x55.
    /// The captured frames are the ones vxe-battery-tray (PROTOCOL.md) and OpenMouse (src/drivers/atk/hid.test.ts) publish.
    /// </summary>
    public sealed class OemMouseTests
    {
        static byte[] Hex(string hex) => hex.Split(' ').Select(b => Convert.ToByte(b, 16)).ToArray();

        /// <summary>A reply with the given head, zero padded, and the checksum that makes it add up to 0x55.</summary>
        static byte[] Reply(params byte[] head)
        {
            var r = new byte[17];
            head.CopyTo(r, 0);
            r[16] = (byte)(0x55 - r.Take(16).Sum(b => b));
            return r;
        }

        [Fact]
        public void RequestIsTheCapturedOne()
        {
            Assert.Equal(Hex("08 04 00 00 00 00 00 00 00 00 00 00 00 00 00 00 49"), Frame(0x08, 0x04));
        }

        [Fact]
        public void VxeR1SePlusOnBattery()
        {
            // 95%, not charging, 4096 mV
            Assert.Equal(new Battery(95, false), Parse(Hex("08 04 00 00 00 02 5F 00 10 00 00 00 00 00 00 00 D8")));
        }

        [Fact]
        public void AtkMouseThroughItsDongle()
        {
            Assert.Equal(new Battery(100, false), Parse(Hex("08 04 00 00 00 02 64 00 10 8E 00 00 00 00 00 00 45")));
        }

        [Fact]
        public void AresonReplyComesAsReport09()
        {
            Assert.Equal(new Battery(75, true), Parse(Reply(0x09, 0x04, 0x00, 0x00, 0x00, 0x02, 0x4B, 0x01)));
        }

        [Fact]
        public void LevelWithoutChargingByte()
        {
            Assert.Equal(new Battery(42, false), Parse(Reply(0x08, 0x04, 0x00, 0x00, 0x00, 0x01, 0x2A, 0x01)));
        }

        [Fact]
        public void WrongChecksumIsNotTheReply()
        {
            byte[] r = Hex("08 04 00 00 00 02 5F 00 10 00 00 00 00 00 00 00 D9");
            Assert.False(IsPowerReply(r));
            Assert.Null(Parse(r));
        }

        [Fact]
        public void ButtonEventIsNotTheReply()
        {
            Assert.False(IsPowerReply(Reply(0x09, 0x0A, 0x00, 0x00, 0x00, 0x01, 0x01)));
        }

        [Fact]
        public void RefusedOrEmptyIsNoReading()
        {
            Assert.Null(Parse(Reply(0x08, 0x04, 0x01)));
            Assert.Null(Parse(Reply(0x08, 0x04, 0x00, 0x00, 0x00, 0x00)));
            Assert.Null(Parse(new byte[] { 0x08, 0x04, 0x00, 0x00, 0x00, 0x02, 0x5F }));
        }

        [Fact]
        public void EmptyOnBatteryIsTheReceiverAnsweringForAnOffMouse()
        {
            Assert.Null(Parse(Reply(0x08, 0x04, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00)));
            Assert.Equal(new Battery(0, true), Parse(Reply(0x08, 0x04, 0x00, 0x00, 0x00, 0x02, 0x00, 0x01)));
        }

        [Fact]
        public void LevelOverAHundredIsNoReading()
        {
            Assert.Null(Parse(Reply(0x08, 0x04, 0x00, 0x00, 0x00, 0x02, 0x65, 0x00)));
        }
    }
}
