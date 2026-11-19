using System;
using System.Linq;
using BatteryTray.Providers;
using Xunit;
using static BatteryTray.Providers.AjazzMouseProvider;

namespace BatteryTray.Tests
{
    /// <summary>
    /// The battery read of the mice on A8A5:2255 / A8A4:2255 (AJAZZ AJ139 V2 MC and others): 64-byte reports with no
    /// report ID, so everything here is one byte further than in the protocol notes, after the ID that Windows adds.
    /// </summary>
    public sealed class AjazzMouseTests
    {
        /// <summary>A report with the report ID 0 and the given bytes after it, zero padded.</summary>
        static byte[] Report(params byte[] body)
        {
            var r = new byte[65];
            body.CopyTo(r, 1);
            return r;
        }

        static byte[] BatteryReply(byte level, byte power) =>
            Report(0xAA, 0x30, 0xA5, 0x00, 0x00, 0x00, 0x00, 0x00, level, power);

        [Fact]
        public void RequestWithTheChecksumOfTheNotes()
        {
            // ajazz-aj139-macos: 55 30 A5 CHK 2E, CHK = the low byte of the sum of bytes 4 to 63
            Assert.Equal(Report(0x55, 0x30, 0xA5, 0x2E, 0x2E), Request(0));
        }

        [Fact]
        public void RequestOfTheWebPanel()
        {
            // OpenMouse, src/ksnake/index.ts: GET_BATTERY with the tail a5 0b 2e 01 01
            Assert.Equal(Report(0x55, 0x30, 0xA5, 0x0B, 0x2E, 0x01, 0x01), Request(1));
        }

        [Fact]
        public void RequestsAreWholeReports()
        {
            Assert.All(new[] { Request(0), Request(1) }, r => Assert.Equal(65, r.Length));
        }

        [Fact]
        public void LevelOnBattery()
        {
            Assert.Equal(new Battery(63, false), Parse(BatteryReply(63, 0)));
        }

        [Fact]
        public void OnUsbPowerItIsCharging()
        {
            Assert.Equal(new Battery(80, true), Parse(BatteryReply(80, 1)));
        }

        [Fact]
        public void FullAndEmptyOnTheCable()
        {
            Assert.Equal(new Battery(100, true), Parse(BatteryReply(100, 1)));
            Assert.Equal(new Battery(0, true), Parse(BatteryReply(0, 1)));
        }

        [Fact]
        public void ZeroFromTheReceiverIsAMouseItCannotReach()
        {
            Assert.Null(Parse(BatteryReply(0, 0)));
        }

        [Fact]
        public void LevelOver100IsNoReading()
        {
            Assert.Null(Parse(BatteryReply(101, 0)));
            Assert.Null(Parse(BatteryReply(0xFF, 0)));
        }

        [Fact]
        public void TheMouseOwnReportsAreNotTheReply()
        {
            Assert.False(IsBatteryReply(Report(0xAA, 0xFA, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40)));
            Assert.False(IsBatteryReply(Report(0xAA, 0xED, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40)));
            Assert.Null(Parse(Report(0xAA, 0xFA, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x40)));
        }

        [Fact]
        public void TheRequestComingBackIsNotTheReply()
        {
            Assert.False(IsBatteryReply(Request(0)));
            Assert.False(IsBatteryReply(Request(1)));
        }

        [Fact]
        public void AShortReportIsNotTheReply()
        {
            Assert.False(IsBatteryReply(Report(0xAA, 0x30).Take(10).ToArray()));
            Assert.False(IsBatteryReply(Array.Empty<byte>()));
        }
    }
}
