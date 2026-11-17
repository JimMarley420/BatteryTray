using System;
using System.Linq;
using BatteryTray.Providers;
using Xunit;
using static BatteryTray.Providers.AulaMchoseProvider;

namespace BatteryTray.Tests
{
    /// <summary>
    /// Keyboards on the CompX receiver 3554:FA09: 20-byte reports 13. The requests are M HUB's, and the two real replies
    /// are the ones a G75 Pro owner captured (myfinder-device-battery issue 476, quoted in its upstream-protocols.md).
    /// </summary>
    public sealed class AulaMchoseTests
    {
        static byte[] Hex(string hex) => hex.Split(' ').Select(b => Convert.ToByte(b, 16)).ToArray();

        static byte[] Frame(params byte[] head)
        {
            var r = new byte[20];
            head.CopyTo(r, 0);
            return r;
        }

        [Fact]
        public void RequestsAreMHubs()
        {
            Assert.Equal(Hex("13 4A 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 5E"), Request(0x4A));
            Assert.Equal(Hex("13 05 01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 19"), Request(0x05));
        }

        [Fact]
        public void CapturedBatteryReply()
        {
            // 100%; the low nibble of byte 6 is "full", not charging
            Assert.Equal(new Battery(100, false), ParseBattery(Hex("13 4A 01 00 02 64 01 00 00 00 00 00 00 00 00 00 00 00 00 00")));
        }

        [Fact]
        public void CapturedModelCodeNamesTheG75Pro()
        {
            Assert.Equal("MCHOSE G75 Pro", ModelName(Hex("13 05 01 00 03 00 00 00 01 15 00 00 00 00 00 00 00 00 00 00")));
        }

        [Fact]
        public void ModelCodeWhereMHubReadsIt()
        {
            Assert.Equal("MCHOSE K99 V2", ModelName(Frame(0x13, 0x05, 0x01, 0x00, 0x06, 0x03, 0x00, 0x00, 0x00, 0x01, 0xDF)));
        }

        [Fact]
        public void UnknownModelCodeIsNoName()
        {
            Assert.Null(ModelName(Frame(0x13, 0x05, 0x01, 0x00, 0x06, 0x07, 0x00, 0x00, 0x00, 0x09, 0x99)));
        }

        [Fact]
        public void Charging()
        {
            // AULA sends 0x10 with the cable in, M HUB's charge flag is the same nibble
            Assert.Equal(new Battery(57, true), ParseBattery(Frame(0x13, 0x4A, 0x01, 0x00, 0x02, 0x39, 0x10)));
        }

        [Fact]
        public void EchoedCommandWithTheTopBitSet()
        {
            Assert.Equal(new Battery(80, false), ParseBattery(Frame(0x13, 0xCA, 0x00, 0x00, 0x02, 0x50, 0x01)));
        }

        [Fact]
        public void OtherReportsAreNotTheReply()
        {
            Assert.False(IsReply(Frame(0x13, 0x0A, 0x01, 0x00, 0x04, 0x05, 0x5F, 0x01), 0x4A)); // status push
            Assert.False(IsReply(Request(0x4A), 0x4A));                                       // the request itself
            Assert.False(IsReply(Hex("13 4A 01 00 02 64"), 0x4A));                           // too short
        }

        [Fact]
        public void NoLevelIsNoReading()
        {
            Assert.Null(ParseBattery(Frame(0x13, 0x4A, 0x01, 0x00, 0x02, 0x00, 0x01)));
            Assert.Null(ParseBattery(Frame(0x13, 0x4A, 0x01, 0x00, 0x02, 0x65, 0x00)));
        }
    }
}
