using System.Linq;
using BatteryTray.Core;
using Xunit;

namespace BatteryTray.Tests
{
    /// <summary>
    /// The order of the devices on the settings tab. It must not change while a name is typed: the list is rebuilt
    /// when its order changes, and that took the focus out of the name box and moved the row. Typing "1" as the name
    /// of "Razer Barracuda X (BT)" put it above "Razer Barracuda X".
    /// </summary>
    public sealed class SettingsOrderTests
    {
        /// <param name="own">The name the device gives itself.</param>
        /// <param name="shown">The name the user gave it, if any.</param>
        static DeviceRow Row(string id, DeviceKind kind, string own, string? shown = null)
        {
            var reading = new DeviceReading { Id = id, Name = own, Kind = kind, Via = "test" };
            return new DeviceRow(id, reading, null, shown ?? own);
        }

        static string[] Ids(params DeviceRow[] rows) => DeviceRow.SettingsOrder(rows).Select(r => r.Id).ToArray();

        [Fact]
        public void ByKindThenByTheDeviceOwnName()
        {
            Assert.Equal(new[] { "headset", "mouse-a", "mouse-b" }, Ids(
                Row("mouse-b", DeviceKind.Mouse, "R3 Mouse"),
                Row("headset", DeviceKind.Headset, "Razer Barracuda X"),
                Row("mouse-a", DeviceKind.Mouse, "Attack Shark R3")));
        }

        [Fact]
        public void ANameBeingTypedDoesNotMoveTheRow()
        {
            DeviceRow dongle = Row("razer:barracuda-x", DeviceKind.Headset, "Razer Barracuda X");
            string[] before = Ids(dongle, Row("bt:445ECD546C71", DeviceKind.Headset, "Razer Barracuda X (BT)"));
            foreach (string typed in new[] { "1", "1 ", "A", "zzz", "razer" })
                Assert.Equal(before, Ids(dongle, Row("bt:445ECD546C71", DeviceKind.Headset, "Razer Barracuda X (BT)", typed)));
            Assert.Equal(new[] { "razer:barracuda-x", "bt:445ECD546C71" }, before);
        }

        [Fact]
        public void TheSameOwnNameGoesById()
        {
            Assert.Equal(new[] { "ry:a", "ry:b" }, Ids(
                Row("ry:b", DeviceKind.Mouse, "Mouse", "First"),
                Row("ry:a", DeviceKind.Mouse, "Mouse", "Second")));
        }

        [Fact]
        public void CaseDoesNotCount()
        {
            Assert.Equal(new[] { "a", "b" }, Ids(
                Row("b", DeviceKind.Mouse, "vxe r1"),
                Row("a", DeviceKind.Mouse, "Attack Shark R3")));
        }

        [Fact]
        public void ARememberedDeviceGoesByItsLastReading()
        {
            var last = new DeviceReading { Id = "old", Name = "Aerox 3", Kind = DeviceKind.Mouse, Via = "test" };
            Assert.Equal(new[] { "old", "new" }, Ids(
                Row("new", DeviceKind.Mouse, "R3 Mouse"),
                new DeviceRow("old", null, last, "Zzz")));
        }
    }
}
