using BatteryTray.Providers;
using Xunit;
using static BatteryTray.Providers.HeadsetProvider;

namespace BatteryTray.Tests
{
    /// <summary>
    /// Corsair's Bragi property replies as the Virtuoso XT / SE and the HS80 RGB Wireless send them on report 01:
    /// <c>01 FF 02 SS VV VV VV VV</c>, with the layout from HeadsetControl (corsair_virtuoso_xt.hpp) and OpenLinkHub.
    /// The report ID comes first, so nothing is stripped in front of it.
    /// </summary>
    public sealed class CorsairBragiTests
    {
        const byte Receiver = 0x09, Self = 0x08;

        static byte[] Reply(params byte[] data)
        {
            var report = new byte[64];
            data.CopyTo(report, 0);
            return report;
        }

        [Fact]
        public void HeadsetBehindTheReceiverAnswers()
        {
            // 87.3% = 873 = 0x0369
            byte[] r = Reply(0x01, 0x01, 0x02, 0x00, 0x69, 0x03);
            Assert.True(IsBragiReply(r, Receiver));
            Assert.Equal(873u, BragiValue(r));
        }

        [Fact]
        public void HeadsetOnItsCableAnswersForItself()
        {
            byte[] r = Reply(0x01, 0x00, 0x02, 0x00, 0xE8, 0x03);
            Assert.True(IsBragiReply(r, Self));
            Assert.False(IsBragiReply(r, Receiver));
            Assert.Equal(1000u, BragiValue(r));
        }

        [Fact]
        public void ReceiverAnsweringForItselfIsNotTheHeadset()
        {
            Assert.False(IsBragiReply(Reply(0x01, 0x00, 0x02, 0x00, 0x1C, 0x1B), Receiver));
        }

        [Fact]
        public void NoSuchPropertyIsNoValue()
        {
            Assert.Null(BragiValue(Reply(0x01, 0x01, 0x02, 0x05)));
        }

        [Fact]
        public void OtherReportsAreNotTheReply()
        {
            Assert.False(IsBragiReply(Reply(0x0E, 0x01), Receiver));               // volume event
            Assert.False(IsBragiReply(Reply(0x01, 0x01, 0x01, 0x00), Receiver));   // reply to a write
            Assert.False(IsBragiReply(new byte[] { 0x01, 0x01, 0x02, 0x00 }, Receiver)); // too short
        }

        [Fact]
        public void LevelIsInTenthsOfAPercent()
        {
            Assert.Equal(new State(true, 87, false), BragiState(873, 2));
            Assert.Equal(new State(true, 0, false), BragiState(9, 2));
            Assert.Equal(new State(true, 100, false), BragiState(1000, null));
        }

        [Fact]
        public void ChargingAndCharged()
        {
            Assert.Equal(new State(true, 54, true), BragiState(540, 1));
            Assert.Equal(new State(true, 100, false, Full: true), BragiState(1000, 3));
        }

        [Fact]
        public void LevelOutOfRangeIsNotKnown()
        {
            Assert.Equal(new State(true, null, true, "d.levelUnknown"), BragiState(0x0A69, 1));
        }
    }
}
