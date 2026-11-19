using System.Collections.Generic;
using BatteryTray.Hid;
using BatteryTray.Providers;
using Xunit;
using static BatteryTray.Providers.OemMouseProvider;

namespace BatteryTray.Tests
{
    /// <summary>
    /// A collection that Windows won't open comes from the enumeration with nothing but its path and IDs: no usage
    /// page, no report lengths. So it can't be picked as a device's vendor collection, and a device whose collection
    /// is missing while one of them is locked needs administrator rights, not "couldn't reach the device".
    /// </summary>
    public sealed class HidCollectionsTests
    {
        static HidInfo Collection(int mi, ushort page, ushort usage, int input = 0, int output = 0, int feature = 0) => new()
        {
            Path = $@"\\?\hid#vid_25a7&pid_fa7c&mi_{mi:x2}&col01#8&1&0&0000#{{4d1e55b2-f16f-11cf-88cb-001111000030}}",
            VendorId = 0x25A7, ProductId = 0xFA7C, UsagePage = page, Usage = usage,
            InputLength = input, OutputLength = output, FeatureLength = feature,
        };

        static HidInfo Locked(int mi) => new()
        {
            Path = $@"\\?\hid#vid_25a7&pid_fa7c&mi_{mi:x2}&col02#8&1&0&0001#{{4d1e55b2-f16f-11cf-88cb-001111000030}}",
            VendorId = 0x25A7, ProductId = 0xFA7C, AccessDenied = true,
        };

        static readonly HidInfo Mouse = Collection(0, 0x0001, 0x0002, input: 8);

        [Fact]
        public void NothingLockedIsNoLink()
        {
            Assert.Equal("d.noLink", HidCollections.NoLinkReason(new[] { Mouse }));
            Assert.Equal("d.noLink", HidCollections.NoLinkReason(new List<HidInfo>()));
        }

        [Fact]
        public void ALockedCollectionNeedsAdmin()
        {
            Assert.Equal("d.needsAdmin", HidCollections.NoLinkReason(new[] { Mouse, Locked(1) }));
        }

        [Fact]
        public void VxeAsksAndListensOnOneCollection()
        {
            HidInfo vendor = Collection(1, 0xFF02, 0x0002, input: 17, output: 17);
            var (send, listen) = Channels(Transport.Output, new List<HidInfo> { Mouse, vendor });
            Assert.Same(vendor, send);
            Assert.Same(vendor, listen);
        }

        [Fact]
        public void AresonAsksOnOneCollectionAndListensOnAnother()
        {
            HidInfo command = Collection(1, 0xFF02, 0x0002, feature: 17);
            HidInfo reply = Collection(1, 0xFF01, 0x0000, input: 17);
            var (send, listen) = Channels(Transport.Feature, new List<HidInfo> { Mouse, reply, command });
            Assert.Same(command, send);
            Assert.Same(reply, listen);
        }

        [Fact]
        public void ALockedCollectionIsNeverPicked()
        {
            var locked = new List<HidInfo> { Mouse, Locked(1) };
            Assert.Equal((null, null), Channels(Transport.Output, locked));
            Assert.Equal((null, null), Channels(Transport.Feature, locked));
            Assert.Null(AulaMchoseProvider.VendorCollection(locked));
        }

        [Fact]
        public void AresonWithItsReplyCollectionLocked()
        {
            HidInfo command = Collection(1, 0xFF02, 0x0002, feature: 17);
            var collections = new List<HidInfo> { Mouse, command, Locked(1) };
            var (send, listen) = Channels(Transport.Feature, collections);
            Assert.Same(command, send);
            Assert.Null(listen);
            Assert.Equal("d.needsAdmin", HidCollections.NoLinkReason(collections));
        }

        [Fact]
        public void AresonReplyCollectionOfAnotherInterfaceIsNotTheOne()
        {
            HidInfo command = Collection(1, 0xFF02, 0x0002, feature: 17);
            HidInfo other = Collection(2, 0xFF01, 0x0000, input: 17);
            var (send, listen) = Channels(Transport.Feature, new List<HidInfo> { command, other });
            Assert.Same(command, send);
            Assert.Null(listen);
        }

        [Fact]
        public void KeyboardReceiverVendorCollection()
        {
            HidInfo vendor = Collection(1, 0xFF02, 0x0002, input: 20, output: 20);
            HidInfo tooShort = Collection(1, 0xFF02, 0x0002, input: 8, output: 8);
            Assert.Same(vendor, AulaMchoseProvider.VendorCollection(new List<HidInfo> { Mouse, tooShort, vendor }));
            Assert.Null(AulaMchoseProvider.VendorCollection(new List<HidInfo> { Mouse, tooShort }));
        }
    }
}
