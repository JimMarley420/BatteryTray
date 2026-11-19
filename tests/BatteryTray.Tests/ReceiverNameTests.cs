using BatteryTray.Core;
using BatteryTray.Providers;
using Xunit;

namespace BatteryTray.Tests
{
    /// <summary>
    /// Mouse names from the product string of a receiver that several brands share. "AJAZZ 2.4G 8K" is the AJAZZ
    /// AJ179 Pro's receiver 3151:402D (issue #22), "2.4G Wireless Mouse" the Attack Shark R3's with the same IDs,
    /// "AJAZZ 2.4G" the AJ139 V2 MC's A8A5:2255 (issue #20).
    /// </summary>
    public sealed class ReceiverNameTests
    {
        public ReceiverNameTests()
        {
            Loc.Set("en");
        }

        [Theory]
        [InlineData("AJAZZ 2.4G 8K", "AJAZZ")]
        [InlineData("AJAZZ 2.4G", "AJAZZ")]
        [InlineData("2.4G Wireless Mouse", "")]
        [InlineData("2.4G Wireless Receiver", "")]
        [InlineData("  usb   DONGLE ", "")]
        [InlineData("2.4G Wireless Gaming Mouse", "")]
        [InlineData("USB Gaming Keyboard", "")]
        [InlineData("MCHOSE G7 Pro", "MCHOSE G7 Pro")]
        [InlineData("MonsGeek 2.4G Wireless Keyboard", "MonsGeek")]
        [InlineData("", "")]
        [InlineData(null, "")]
        public void BrandIsWhatIsLeftWithoutThePlainWords(string? product, string brand)
        {
            Assert.Equal(brand, ReceiverName.Brand(product));
        }

        [Fact]
        public void OneWordIsABrand()
        {
            Assert.Equal("AJAZZ mouse", ReceiverName.Mouse("AJAZZ 2.4G 8K", "Attack Shark R3"));
        }

        [Fact]
        public void SeveralWordsAreTheName()
        {
            Assert.Equal("MCHOSE G7 Pro", ReceiverName.Mouse("MCHOSE G7 Pro"));
        }

        [Fact]
        public void AStringThatNamesNobodyGivesTheFallback()
        {
            Assert.Equal("Attack Shark R3", ReceiverName.Mouse("2.4G Wireless Mouse", "Attack Shark R3"));
            Assert.Equal("Attack Shark R3", ReceiverName.Mouse(null, "Attack Shark R3"));
        }

        [Fact]
        public void WithoutAFallbackItIsJustAMouse()
        {
            Assert.Equal("Mouse", ReceiverName.Mouse("2.4G Wireless Receiver"));
        }
    }
}
