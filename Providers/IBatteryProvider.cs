using System;
using System.Collections.Generic;
using System.Linq;
using BatteryTray.Core;
using BatteryTray.Hid;

namespace BatteryTray.Providers
{
    public interface IBatteryProvider
    {
        string Name { get; }

        /// <summary>Queries the hardware (blocking; called on a worker thread) and returns every device this
        /// provider currently knows, online or not.</summary>
        IReadOnlyList<DeviceReading> Poll();

        /// <summary>How long to wait before the next poll, given what the last one returned.</summary>
        TimeSpan Delay(IReadOnlyList<DeviceReading> last);
    }

    /// <summary>A provider whose devices also report changes by themselves: it asks to be polled right away.</summary>
    public interface IPushingProvider
    {
        event Action? Pushed;
    }

    /// <summary>What the providers share when they look for a device's HID collection.</summary>
    internal static class HidCollections
    {
        /// <summary>
        /// The text key for a device whose collection wasn't found among <paramref name="collections"/>. A collection
        /// that Windows won't open comes with no usage and no report lengths, so it never matches what a provider
        /// looks for. When there is one, it is most likely the one that's missing, and administrator rights open it.
        /// </summary>
        internal static string NoLinkReason(IEnumerable<HidInfo> collections) =>
            collections.Any(i => i.AccessDenied) ? "d.needsAdmin" : "d.noLink";
    }

    public static class ProviderList
    {
        public static IBatteryProvider[] Create(StateStore state)
        {
            return new IBatteryProvider[]
            {
                new RazerBarracudaProvider(state),
                new RongyuanDongleProvider(state),
                new XentaReceiverProvider(state),
                new SteelSeriesProvider(),
                new OemMouseProvider(),
                new AjazzMouseProvider(),
                new AulaMchoseProvider(),
                new SonixKeyboardProvider(),
                new RazerPeripheralProvider(),
                new HeadsetProvider(),
                new LogitechProvider(),
                new PlayStationProvider(),
                new XInputProvider(),
                new BluetoothProvider(),
                new StandardHidBatteryProvider(),
            };
        }
    }
}
