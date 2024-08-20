# raft-implementation

## Summary

### What Works

So far I have what I would consider a "first draft" impl written. It "works". 

And what I mean by that is that I can start up 3 nodes manually (on one machine using localhost for all of them) and they will be able to elect a leader and then replicate values. I can also crash the leader and the two remaining nodes will keep going. Bringing the original node back online is a little murky. Doesn't really work, it crashes often and it will never get new logs either. Also, write requests have to be manually set to go to the leader. The cluster won't redirect you and some weird stuff happens if you send writes to a non-leader node. 

### "In progress"
Better way to put it is things I've started but not finished

1. Adding risdb-proxy. I started messing around with the CloudFlare proxy crate pingora to try and get something that would proxy to the right node for read/write requests. This stalled and isn't currently being worked on
2. Create a CLI for deploying a cluster of nodes. Currently working on doing this for local testing with a loftier goal of getting it to work in the cloud as well. 

### Long term goals

A long shot goal is some form of testing to be added so I can "verify" that this works. Unsure if I'll get there though but I would at least like to do the other steps. 

## Development

### Sharp edges

Before running the `helloworld-server` binary you need to have run the `./refresh-certificates.sh` script first. This generates the certs that the Raft frontend (risdb-hyper) will use. If you don't do this you'll get a cryptic error message about trying to drop a Tokio runtime from within a tokio runtime. (TODO: Understand why this is the error that occurs and how to make it more obvious what needs to be done to fix it) 
