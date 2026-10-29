{-# LANGUAGE ScopedTypeVariables #-}
import Transient.Base
import Transient.Internals
import Transient.Indeterminism
import Control.Monad.IO.Class
import Control.Concurrent

main = do
  putStrLn "--- 1: one registration, three threads in parallel"
  r <- keep' $ do
     onFinish $ \_ -> liftIO (putStrLn "FINISH")
     i <- choose [1,2,3::Int]
     liftIO $ threadDelay (i*20000)
     liftIO $ putStrLn ("work " ++ show i)
  print (r :: [()])

  putStrLn "--- 2: the onFinish statement runs for each result"
  r <- keep' $ do
     i <- choose [1,2,3::Int]
     onFinish $ \_ -> liftIO (putStrLn ("FINISH " ++ show i))
     liftIO $ threadDelay (i*20000)
     liftIO $ putStrLn ("work " ++ show i)
  print (r :: [()])
