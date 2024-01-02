# atto

This is just some code I had written to improve my understanding of atomics, starting with building a simple channel implementation, moving onto more complex stuff.

## Channel
A shared handle to a `VecDeque` allows the user to send data from Sender side and receive the same from the Receiver side. Uses `AtomicU32` to keep track of length and `AtomicUsize` for the number of senders(i.e. as many senders as system can address).

I am yet to figure out the safety implications of mutably accessing the queue in parallel operations and would love to get resources to read up on the same, please feel free to forward them to `devdutt [at] outlook [dot] in`.