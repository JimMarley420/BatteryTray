using System;
using System.Collections.Generic;
using System.Linq;
using BatteryTray.Core;

namespace BatteryTray.Providers
{
    /// <summary>
    /// Names for devices behind a receiver whose USB IDs several brands share. The battery reply doesn't say which
    /// device it is, so all that tells them apart is the receiver's product string: "AJAZZ 2.4G 8K" on the AJAZZ
    /// AJ179 Pro, "2.4G Wireless Mouse" on the Attack Shark R3, both on 3151:402D (issue #22).
    /// </summary>
    internal static class ReceiverName
    {
        /// <summary>Words that say how a device is connected or what kind it is, not whose it is.</summary>
        static readonly HashSet<string> PlainWords = new(StringComparer.OrdinalIgnoreCase)
        {
            "2.4G", "2.4GHz", "1K", "2K", "4K", "8K", "USB", "HID", "Wireless", "Receiver", "Dongle", "Mouse",
            "Keyboard", "Device", "Gaming", "Optical",
        };

        /// <summary>What the product string says besides such words: "AJAZZ 2.4G 8K" -> "AJAZZ", "2.4G Wireless
        /// Mouse" -> "".</summary>
        internal static string Brand(string? product)
        {
            IEnumerable<string> words = (product ?? "").Split((char[]?)null, StringSplitOptions.RemoveEmptyEntries);
            return string.Join(' ', words.Where(w => !PlainWords.Contains(w)));
        }

        /// <summary>
        /// The name of the mouse behind a receiver with this product string. One word is taken for a brand and shown
        /// as "AJAZZ mouse", more are shown as they are ("MCHOSE G7 Pro"). A string that names nobody gives
        /// <paramref name="fallback"/>, or just "Mouse".
        /// </summary>
        internal static string Mouse(string? product, string? fallback = null)
        {
            string brand = Brand(product);
            if (brand.Length == 0) return fallback ?? Loc.T("name.mouse");
            return brand.Contains(' ') ? brand : Loc.T("name.brandMouse", brand);
        }

        /// <summary>The same for a keyboard: "AJAZZ keyboard", "Neo Ergo", or <paramref name="fallback"/>.</summary>
        internal static string Keyboard(string? product, string? fallback = null)
        {
            string brand = Brand(product);
            if (brand.Length == 0) return fallback ?? Loc.T("name.keyboard");
            return brand.Contains(' ') ? brand : Loc.T("name.brandKeyboard", brand);
        }
    }
}
