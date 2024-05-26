use std::{net::SocketAddr, sync::{Arc, Mutex}, time::Duration};

use tokio::time::sleep;
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
pub struct RisDbToDelete {
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
impl RisDbToDelete {
    /**
     * Creates a new RisDbImpl struct
     */
    pub fn new(addr: SocketAddr) -> RisDbToDelete {
        RisDbToDelete { 
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
    #[tracing::instrument]
    pub async fn init_client(
        self, 
        addr: SocketAddr
    ) -> Result<(), RisDbSetupError> {

        tracing::info!("Initializing Raft client...");

        sleep(Duration::from_secs(5)).await;
        
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
impl RisDb for RisDbToDelete {
    #[tracing::instrument(
        skip_all,
        name = "RisDb:put",
        fields(request_id) 
        ret, 
        err
    )]
    async fn put(
        &self,
        request: Request<PutRequest>,
    ) -> Result<Response<PutResponse>, Status> {
        let put_request = request.into_inner();
        tracing::Span::current().record("request_id", &put_request.request_id);

        tracing::info!("Putting values...");

        let mut client = self.client()?;

        let response = client.propose_value(ProposeValueInput {
            request_id: put_request.request_id.clone(),
            values: put_request.values,
        }).await.map_err(|e| {
            tracing::error!(?e, "Failed to put values");
            Status::internal(e.message())
        })?
        .into_inner();

        let reply = PutResponse {
            request_id: put_request.request_id,
            success: response.successful,
        };

        tracing::info!("Successfully put values");

        Ok(Response::new(reply))
    }
    
    //  TODO: I plan to completely remove this whole implementation and replace it with a lighter weight raw hyper frontend
    //   for receiving requests. I probably won't go too crazy with that as far as supporting backwards compatibility (i.e 
    //   it will accept one type of payload and one type of payload only) 
    #[tracing::instrument(
        skip_all,
        name = "RisDb:get",
        fields(request_id) 
        ret, 
        err
    )]
    async fn get(
        &self,
        request: Request<GetRequest>,
    ) -> Result<Response<GetResponse>, Status> {
        let get_request = request.into_inner();
        tracing::Span::current().record("request_id", &get_request.request_id);

        tracing::info!("Getting values...");

        let mut client = self.client()?;

        let response = client.get_value(GetValueInput {
            request_id: get_request.request_id.clone(),
            keys: get_request.keys
        }).await.map_err(|e| {
            tracing::error!(?e, "Failed to get values");
            Status::internal(e.message())
        })?;

        let reply = GetResponse {
            request_id: get_request.request_id,
            success: true,
            values: response.into_inner().values,
        };

        tracing::info!("Successfully got values");

        Ok(Response::new(reply))
    }
}