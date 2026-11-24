// The Rust core (macos/core): commands in, answers out, both JSON. See macos/core/src/lib.rs.
#ifndef BATTERYTRAY_CORE_H
#define BATTERYTRAY_CORE_H

void bt_init(void (*notify)(void));
char *bt_call(const char *command);
void bt_free(char *text);

#endif
