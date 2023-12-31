# raft-implementation
Working on implementing raft

So far I have what I would consider a "first draft" impl written. It "works". 

And what I mean by that is that I can start up 3 nodes manually (on one machine using localhost for all of them) and they will be able to elect a leader and then replicate values. I can also crash the leader and the two remaining nodes will keep going. Bringing the original node back online is a little murky. Doesn't really work, it crashes often and it will never get new logs either. 

Continuing to work on it but I'm happy with the progress so far. Beyond just making it work over local host as Raft is inteded to (i.e makingit so bringing the server back online works without issue, servers catch back up, etc) my plans are to eventually make it so they communicate over an actual network and not just all run on the same machine. Theoretically that should work now but I haven't fully tested it yet. The idea is to do this using EC2 instances that I just spin up and down and can crash the server code manually for some testing to take place. I also plan to make it so that they write to disk rather than just using memory. 

A long shot goal is some form of testing to be added so I can "verify" that this works. Unsure if I'll get there though but I would at least like to do the other steps. 
