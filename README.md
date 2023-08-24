# raft-implementation
Working on implementing raft

## Personal Notes

* Routing to the right leader node. If a client makes a request to write and it goes to a follower node
they need to be re-directed somehow to the leader node. I want to put my Fargate tasks in private subnets so I'm not 100% sure how to do this. 
    * ~Seems like the client code has to be relatively involved to support things like this.~ 
    * Actually I think the best plan of action is as follows: 
        * Have all the fargate tasks running in a private subnet. 
        * Create an ALB with two target groups: 
            * 1 target group points just to the leader
            * 1 target group points to all the nodes
        * Have URL based routing that will route write requests to the leader target group and everything else to the other target group. 
        * Every time that a new leader is determine just need to update this ALB
    * Using ALB scares me cause it can get pretty expensive. I'll just have to set alarms if it starts getting crazy traffic. Or just set it up for tests and then tear it down after. 
        * I'm probably overthinking this and will just have to come back to this when I'm more awake
