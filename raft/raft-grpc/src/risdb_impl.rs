use std::{net::SocketAddr, sync::{Arc, Mutex}};

use tonic::{Request, Response, Status, transport::Channel};

use crate::{risdb::{PutRequest, PutResponse, GetRequest, GetResponse, ris_db_server::RisDb}, raft_grpc::{raft_internal_client::RaftInternalClient, GetValueInput, ProposeValueInput}};

// Errors
#[derive(Debug)]
pub enum RisDbSetupError {
    FailedToConnectToRaft(tonic::transport::Error)
}

impl From<tonic::transport::Error> for RisDbSetupError {
    fn from(err: tonic::transport::Error) -> RisDbSetupError {
        RisDbSetupError::FailedToConnectToRaft(err)
    }
}

// Structs

#[derive(Debug)]
pub struct RisDbImpl {
    // Socket Address of the current server
    pub addr: SocketAddr,
    // Client to make loopback calls to the RaftImpl on this same server. Cloning this is cheap
    pub raft_client: RaftClient,
}

#[derive(Debug, Clone)]
pub struct RaftClient {
    pub client: Arc<Mutex<Option<RaftInternalClient<Channel>>>>,
}


// Implementations
impl RisDbImpl {
    /**
     * Creates a new RisDbImpl struct
     */
    pub fn new(addr: SocketAddr) -> RisDbImpl {
        RisDbImpl { 
            addr: addr,
            raft_client: RaftClient { client: Arc::new(Mutex::new(None)) }
        }
    }

    /**
     * Gets an instance of the client to use. If the client has not been initialized an error will be returned
     * TODO: Make it so this initializes the client if it isn't already 
     */
    pub fn client(&self) -> Result<RaftInternalClient<Channel>, Status> { 
        Ok(
            self.raft_client.client.lock().unwrap()
                .as_ref()
                .ok_or_else(|| Status::failed_precondition("No Raft client initialized"))?
                .clone()
        )
    }
}

impl RaftClient {
    /**
     * Initializes the client
     */
    pub async fn init_client(
        self, 
        addr: SocketAddr
    ) -> Result<(), RisDbSetupError> {
        let raft_client = RaftInternalClient::connect(
            format!("http://[::1]:{}", addr.port())
        ).await?;
        {
            let mut client = self.client.lock().unwrap();
            *client = Some(raft_client);
        }
        Ok(())
    }
}

#[tonic::async_trait] 
impl RisDb for RisDbImpl {
    async fn put(
        &self,
        request: Request<PutRequest>,
    ) -> Result<Response<PutResponse>, Status> {
        println!("RisDb got a put request: {:?}", request);

        let put_request = request.into_inner();

        let mut client = self.client()?;

        let _response = client.propose_value(ProposeValueInput {
            values: put_request.values,
        }).await.map_err(|e| {
            println!("Failed to put values: {:?}", e);
            Status::internal(e.message())
        })?;

        let reply = PutResponse {
            request_id: put_request.request_id,
            success: true,
        };

        Ok(Response::new(reply))
    }

    async fn get(
        &self,
        request: Request<GetRequest>,
    ) -> Result<Response<GetResponse>, Status> {
        println!("RisDb got a get request: {:?}", request);

        let get_request = request.into_inner();

        let mut client = self.client()?;

        let response = client.get_value(GetValueInput {
            keys: get_request.keys
        }).await.map_err(|e| {
            println!("Failed to get values: {:?}", e);
            Status::internal(e.message())
        })?;

        let reply = GetResponse {
            request_id: get_request.request_id,
            success: true,
            values: response.into_inner().values,
        };

        Ok(Response::new(reply))
    }
}