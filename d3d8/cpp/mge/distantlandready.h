#pragma once

#include <deque>

// The event distantLandReady of MGE XE for mods. The renderer pushes a reason here: the
// Direct3D 9 path itself, the wgpu renderer through a message of the helper
// (MGE_StreamMessage). The Lua of MGE XE takes them with MGE_DistantLandReadyNext (apiffi.cpp)
// and triggers the event.
namespace DistantLandReady {
    enum Reason {
        // The distant land of the session is loaded.
        Loaded = 1,
        // After a game load or a jump of the camera, the distant statics around the camera
        // are there (or the wait reached its limit).
        Arrived = 2,
    };

    inline std::deque<int>& queue() {
        static std::deque<int> reasons;
        return reasons;
    }

    // At most 16 wait: Lua that does not take them does not grow the queue.
    inline void push(Reason reason) {
        if (queue().size() < 16) {
            queue().push_back(reason);
        }
    }

    // The next reason, or 0.
    inline int next() {
        if (queue().empty()) {
            return 0;
        }
        const int reason = queue().front();
        queue().pop_front();
        return reason;
    }
}
